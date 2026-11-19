//! Release-source identity validation used by the tag-time workflow gate.
//!
//! The approved values come from the release environment handoff, not from
//! the candidate tree. The command deliberately writes only a sanitized
//! identity record: it never copies credentials or token material.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::Args;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const EXPECTED_PUBLISHABLE_PACKAGES: &[&str] = &[
    "shipper",
    "shipper-cargo-failure",
    "shipper-cli",
    "shipper-config",
    "shipper-core",
    "shipper-duration",
    "shipper-encrypt",
    "shipper-output-sanitizer",
    "shipper-registry",
    "shipper-retry",
    "shipper-sparse-index",
    "shipper-types",
    "shipper-webhook",
];

const AUTH_POSTURES: &[&str] = &["trusted_publishing", "fallback_secret"];
const IDENTITY_SCHEMA_VERSION: &str = "shipper.release_identity.v1";

#[derive(Args, Debug)]
pub struct ReleaseIdentityArgs {
    /// SHA approved by the release-authority GO record.
    #[arg(long)]
    pub approved_sha: String,

    /// Tree approved by the release-authority GO record.
    #[arg(long)]
    pub approved_tree: String,

    /// Workspace version approved by the release-authority GO record.
    #[arg(long)]
    pub approved_version: String,

    /// Tag being validated. Required for tag-triggered runs.
    #[arg(long)]
    pub tag: Option<String>,

    /// Date recorded by the tag ref, used to bind CHANGELOG.md to the tag.
    #[arg(long)]
    pub tag_date: Option<String>,

    /// SHA returned for the current release-authority main ref.
    #[arg(long)]
    pub main_sha: Option<String>,

    /// Stable reference to the external approval record.
    #[arg(long)]
    pub approval_record_ref: String,

    /// Digest or immutable identifier for the external approval record.
    #[arg(long)]
    pub approval_record_sha: String,

    /// Registry posture approved for the release.
    #[arg(long, default_value = "crates-io")]
    pub registry: String,

    /// Authentication posture approved for the release.
    #[arg(long)]
    pub auth_posture: String,

    /// Previously retained release identity artifact to validate, such as a
    /// downloaded `.shipper/release-identity.json` during resume.
    #[arg(long)]
    pub artifact: Option<PathBuf>,

    /// Path for the sanitized identity artifact emitted after validation.
    #[arg(long, default_value = "target/policy/release-identity.json")]
    pub output: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReleaseIdentity {
    pub schema_version: String,
    pub approved_version: String,
    pub approved_sha: String,
    pub approved_tree: String,
    pub tag: Option<String>,
    pub registry: String,
    pub auth_posture: String,
    pub approval_record_ref: String,
    pub approval_record_sha: String,
}

pub fn validate(args: ReleaseIdentityArgs) -> Result<()> {
    validate_inputs(&args)?;

    let current_sha = git_output(["rev-parse", "HEAD"])?;
    let current_tree = git_output(["rev-parse", "HEAD^{tree}"])?;
    require_equal("checked-out commit", &current_sha, &args.approved_sha)?;
    require_equal("checked-out tree", &current_tree, &args.approved_tree)?;

    if let Some(main_sha) = &args.main_sha {
        require_equal("release-authority main", main_sha, &args.approved_sha)?;
    }

    if let Some(tag) = &args.tag {
        validate_tag(tag, &args.approved_version)?;
        let tag_sha = git_output(["rev-list", "-n", "1", &format!("refs/tags/{tag}")])?;
        require_equal("tag commit", &tag_sha, &args.approved_sha)?;
    }

    validate_workspace(&args.approved_version, args.tag_date.as_deref())?;

    let mut identity = ReleaseIdentity {
        schema_version: IDENTITY_SCHEMA_VERSION.to_string(),
        approved_version: args.approved_version.clone(),
        approved_sha: args.approved_sha.clone(),
        approved_tree: args.approved_tree.clone(),
        tag: args.tag.clone(),
        registry: args.registry.clone(),
        auth_posture: args.auth_posture.clone(),
        approval_record_ref: args.approval_record_ref.clone(),
        approval_record_sha: args.approval_record_sha.clone(),
    };

    if let Some(artifact) = &args.artifact {
        let content = fs::read_to_string(artifact)
            .with_context(|| format!("reading release identity artifact {}", artifact.display()))?;
        let retained: ReleaseIdentity = serde_json::from_str(&content)
            .with_context(|| format!("parsing release identity artifact {}", artifact.display()))?;
        let retained_tag = retained
            .tag
            .as_deref()
            .context("release identity artifact is missing its source tag")?;
        validate_tag(retained_tag, &args.approved_version)?;
        if args.tag.is_none() {
            identity.tag = retained.tag.clone();
        }
        validate_retained_identity(&retained, &identity, artifact, &args.approved_version)?;
    }

    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "creating release identity output directory {}",
                parent.display()
            )
        })?;
    }
    let json = serde_json::to_string_pretty(&identity).context("serializing release identity")?;
    fs::write(&args.output, format!("{json}\n"))
        .with_context(|| format!("writing release identity {}", args.output.display()))?;

    println!(
        "release identity: PASS (version {}; commit {}; tree {}; registry {}; auth posture {})",
        identity.approved_version,
        identity.approved_sha,
        identity.approved_tree,
        identity.registry,
        identity.auth_posture
    );
    Ok(())
}

fn validate_inputs(args: &ReleaseIdentityArgs) -> Result<()> {
    validate_hex("approved commit", &args.approved_sha)?;
    validate_hex("approved tree", &args.approved_tree)?;
    if args.approved_version.trim().is_empty() {
        bail!("approved version must not be blank");
    }
    if args.approval_record_ref.trim().is_empty() {
        bail!("approval record reference must not be blank");
    }
    validate_hex_or_opaque("approval record identifier", &args.approval_record_sha)?;
    if args.registry != "crates-io" {
        bail!("release registry must be crates-io, got {}", args.registry);
    }
    if !AUTH_POSTURES.contains(&args.auth_posture.as_str()) {
        bail!(
            "unsupported release auth posture {}; expected one of {}",
            args.auth_posture,
            AUTH_POSTURES.join(", ")
        );
    }
    if let Some(main_sha) = &args.main_sha {
        validate_hex("release-authority main", main_sha)?;
    }
    if args.tag.is_some() && args.tag_date.is_none() {
        bail!("tag-triggered validation requires --tag-date");
    }
    Ok(())
}

fn validate_hex(label: &str, value: &str) -> Result<()> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{label} must be a 40-character hexadecimal SHA");
    }
    Ok(())
}

fn validate_hex_or_opaque(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() || value.chars().any(char::is_whitespace) {
        bail!("{label} must be a nonblank immutable identifier");
    }
    Ok(())
}

fn require_equal(label: &str, actual: &str, expected: &str) -> Result<()> {
    if actual != expected {
        bail!("{label} mismatch: expected {expected}, found {actual}");
    }
    Ok(())
}

fn validate_tag(tag: &str, version: &str) -> Result<()> {
    let expected = format!("v{version}");
    if tag != expected {
        bail!("tag {tag} does not match approved version {version}; expected {expected}");
    }
    Ok(())
}

fn validate_retained_identity(
    retained: &ReleaseIdentity,
    approved: &ReleaseIdentity,
    artifact: &Path,
    approved_version: &str,
) -> Result<()> {
    if retained.schema_version != IDENTITY_SCHEMA_VERSION {
        bail!(
            "release identity artifact {} has unsupported schema {}; expected {}",
            artifact.display(),
            retained.schema_version,
            IDENTITY_SCHEMA_VERSION
        );
    }
    let retained_tag = retained
        .tag
        .as_deref()
        .context("release identity artifact is missing its source tag")?;
    validate_tag(retained_tag, approved_version)?;
    if retained != approved {
        bail!(
            "release identity artifact {} does not match the approved source, version, registry, or auth posture",
            artifact.display()
        );
    }
    Ok(())
}

fn validate_workspace(version: &str, tag_date: Option<&str>) -> Result<()> {
    let metadata = cargo_metadata()?;
    let mut publishable = BTreeMap::new();
    let packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .context("cargo metadata did not contain packages")?;

    for package in packages {
        let name = package
            .get("name")
            .and_then(Value::as_str)
            .context("cargo metadata package missing name")?;
        let is_publishable = match package.get("publish") {
            None | Some(Value::Null) => true,
            Some(Value::Array(values)) => !values.is_empty(),
            Some(other) => bail!("unexpected publish field for {name}: {other}"),
        };
        if is_publishable {
            let package_version = package
                .get("version")
                .and_then(Value::as_str)
                .with_context(|| format!("cargo metadata package {name} missing version"))?;
            publishable.insert(name.to_string(), package_version.to_string());
        }
    }

    for package in packages {
        let name = package
            .get("name")
            .and_then(Value::as_str)
            .context("cargo metadata package missing name")?;
        if !publishable.contains_key(name) {
            continue;
        }

        let dependencies = package
            .get("dependencies")
            .and_then(Value::as_array)
            .with_context(|| format!("cargo metadata package {name} missing dependencies"))?;
        for dependency in dependencies {
            let dependency_name = dependency
                .get("name")
                .and_then(Value::as_str)
                .context("cargo metadata dependency missing name")?;
            if dependency.get("kind").is_some_and(|kind| !kind.is_null())
                || dependency_name == "xtask"
                || !publishable.contains_key(dependency_name)
            {
                continue;
            }
            let requirement = dependency
                .get("req")
                .and_then(Value::as_str)
                .with_context(|| format!("dependency {dependency_name} in {name} missing req"))?;
            let expected_requirement = format!("^{version}");
            if requirement != expected_requirement {
                bail!(
                    "workspace dependency {name} -> {dependency_name} requires {requirement}, expected {expected_requirement}"
                );
            }
        }
    }

    let expected = EXPECTED_PUBLISHABLE_PACKAGES
        .iter()
        .map(|name| (*name).to_string())
        .collect::<BTreeSet<_>>();
    let actual = publishable.keys().cloned().collect::<BTreeSet<_>>();
    if actual != expected {
        bail!(
            "publishable package graph differs from the approved 13-package surface: expected {:?}, found {:?}",
            expected,
            actual
        );
    }
    for (name, package_version) in &publishable {
        if package_version != version {
            bail!("publishable package {name} is {package_version}, expected {version}");
        }
    }

    let changelog = fs::read_to_string("CHANGELOG.md").context("reading CHANGELOG.md")?;
    let heading = format!("## [{version}]");
    if !changelog.lines().any(|line| line.starts_with(&heading)) {
        bail!("CHANGELOG.md is missing the final {version} section");
    }
    if let Some(date) = tag_date {
        let dated_heading = format!("## [{version}] - {date}");
        if !changelog.lines().any(|line| line == dated_heading) {
            bail!("CHANGELOG.md is missing the tag-date heading {dated_heading}");
        }
    }

    let release_notes = PathBuf::from(format!("RELEASE_NOTES_v{version}.md"));
    if !release_notes.is_file() {
        bail!(
            "reviewed release notes are missing: {}",
            release_notes.display()
        );
    }

    Ok(())
}

fn cargo_metadata() -> Result<Value> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .context("running cargo metadata")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("parsing cargo metadata")
}

fn git_output<const N: usize>(args: [&str; N]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .context("running git identity command")?;
    if !output.status.success() {
        bail!(
            "git identity command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_args() -> ReleaseIdentityArgs {
        ReleaseIdentityArgs {
            approved_sha: "a".repeat(40),
            approved_tree: "b".repeat(40),
            approved_version: "0.5.0".to_string(),
            tag: None,
            tag_date: None,
            main_sha: None,
            approval_record_ref: "shipper#477".to_string(),
            approval_record_sha: "record-2026-08-04".to_string(),
            registry: "crates-io".to_string(),
            auth_posture: "trusted_publishing".to_string(),
            artifact: None,
            output: PathBuf::from("target/policy/release-identity.json"),
        }
    }

    fn workflow_job(name: &str) -> String {
        let workflow = include_str!("../../.github/workflows/release.yml");
        let lines = workflow.lines().collect::<Vec<_>>();
        let marker = format!("  {name}:");
        let Some(start) = lines.iter().position(|line| *line == marker) else {
            return String::new();
        };
        let end = lines
            .iter()
            .skip(start + 1)
            .position(|line| {
                line.starts_with("  ")
                    && !line.starts_with("    ")
                    && line.ends_with(':')
                    && !line.trim_start().starts_with('#')
            })
            .map_or(lines.len(), |offset| start + 1 + offset);
        lines[start..end].join("\n")
    }

    #[test]
    fn tag_must_match_approved_version() -> Result<()> {
        validate_tag("v0.5.0", "0.5.0")?;
        let error = validate_tag("v0.5.1", "0.5.0").expect_err("mismatched tag must fail");
        assert!(
            error
                .to_string()
                .contains("does not match approved version")
        );
        Ok(())
    }

    #[test]
    fn input_contract_rejects_invalid_registry_auth_and_tag_inputs() -> Result<()> {
        let mut args = fixture_args();
        args.registry = "private-registry".to_string();
        let error = validate_inputs(&args).expect_err("unapproved registry must fail");
        assert!(error.to_string().contains("must be crates-io"));

        let mut args = fixture_args();
        args.auth_posture = "unknown".to_string();
        let error = validate_inputs(&args).expect_err("unapproved auth posture must fail");
        assert!(
            error
                .to_string()
                .contains("unsupported release auth posture")
        );

        let mut args = fixture_args();
        args.tag = Some("v0.5.0".to_string());
        let error = validate_inputs(&args).expect_err("tag without date must fail");
        assert!(error.to_string().contains("requires --tag-date"));
        Ok(())
    }

    #[test]
    fn shas_are_exactly_40_hex_characters() -> Result<()> {
        validate_hex("sha", &"a".repeat(40))?;
        let error = validate_hex("sha", "abc").expect_err("short sha must fail");
        assert!(error.to_string().contains("40-character hexadecimal"));
        Ok(())
    }

    #[test]
    fn opaque_record_identifier_rejects_whitespace() -> Result<()> {
        validate_hex_or_opaque("record", "record-2026-08-04")?;
        let error = validate_hex_or_opaque("record", "record with spaces")
            .expect_err("record identifier with whitespace must fail");
        assert!(error.to_string().contains("immutable identifier"));
        Ok(())
    }

    #[test]
    fn retained_resume_identity_mismatch_fails_closed() -> Result<()> {
        let approved = ReleaseIdentity {
            schema_version: "shipper.release_identity.v1".to_string(),
            approved_version: "0.5.0".to_string(),
            approved_sha: "a".repeat(40),
            approved_tree: "b".repeat(40),
            tag: Some("v0.5.0".to_string()),
            registry: "crates-io".to_string(),
            auth_posture: "trusted_publishing".to_string(),
            approval_record_ref: "shipper#477".to_string(),
            approval_record_sha: "record-2026-08-04".to_string(),
        };
        validate_retained_identity(
            &approved,
            &approved,
            Path::new(".shipper/release-identity.json"),
            "0.5.0",
        )?;
        let mut retained = approved.clone();
        retained.approved_sha = "c".repeat(40);
        let error = validate_retained_identity(
            &retained,
            &approved,
            Path::new(".shipper/release-identity.json"),
            "0.5.0",
        )
        .expect_err("resume identity from another source must fail");
        assert!(
            error
                .to_string()
                .contains("does not match the approved source")
        );
        Ok(())
    }

    #[test]
    fn retained_resume_identity_schema_mismatch_fails_closed() -> Result<()> {
        let mut retained = ReleaseIdentity {
            schema_version: IDENTITY_SCHEMA_VERSION.to_string(),
            approved_version: "0.5.0".to_string(),
            approved_sha: "a".repeat(40),
            approved_tree: "b".repeat(40),
            tag: Some("v0.5.0".to_string()),
            registry: "crates-io".to_string(),
            auth_posture: "trusted_publishing".to_string(),
            approval_record_ref: "shipper#477".to_string(),
            approval_record_sha: "record-2026-08-04".to_string(),
        };
        retained.schema_version = "shipper.release_identity.v2".to_string();
        let error = validate_retained_identity(
            &retained,
            &ReleaseIdentity {
                schema_version: IDENTITY_SCHEMA_VERSION.to_string(),
                ..retained.clone()
            },
            Path::new(".shipper/release-identity.json"),
            "0.5.0",
        )
        .expect_err("unsupported resume artifact schema must fail");
        assert!(error.to_string().contains("unsupported schema"));
        Ok(())
    }

    #[test]
    fn workflow_fixture_scopes_permissions_and_repository_guards() {
        let workflow = include_str!("../../.github/workflows/release.yml");
        let Some(top_level) = workflow.split("jobs:").next() else {
            return;
        };
        assert!(top_level.contains("permissions:\n  contents: read"));
        assert!(!top_level.contains("contents: write"));
        assert!(!top_level.contains("id-token: write"));

        for job in [
            "release-identity-gate",
            "build-binaries",
            "msrv-gate",
            "policy-gate",
            "release-proof-gate",
            "publish-crates-io",
            "create-release",
            "release-rehearse",
            "release-resume",
        ] {
            assert!(
                workflow_job(job).contains("github.repository == 'EffortlessMetrics/shipper'"),
                "release job {job} must retain the release-authority repository guard"
            );
        }
        assert!(workflow_job("publish-crates-io").contains("id-token: write"));
        assert!(workflow_job("release-rehearse").contains("id-token: write"));
        assert!(workflow_job("release-resume").contains("id-token: write"));
        assert!(workflow_job("create-release").contains("contents: write"));
        assert!(!workflow_job("build-binaries").contains("id-token: write"));
        assert!(!workflow_job("release-proof-gate").contains("id-token: write"));
    }

    #[test]
    fn workflow_fixture_requires_identity_proof_and_all_four_binaries() {
        let publish = workflow_job("publish-crates-io");
        assert!(publish.contains("release-identity-gate"));
        assert!(publish.contains("build-binaries"));
        assert!(publish.contains("release-proof-gate"));
        assert!(publish.contains("msrv-gate"));
        assert!(publish.contains("policy-gate"));
        assert!(publish.contains("verify-binaries"));

        let binaries = workflow_job("build-binaries");
        for target in [
            "x86_64-unknown-linux-gnu",
            "x86_64-apple-darwin",
            "aarch64-apple-darwin",
            "x86_64-pc-windows-msvc",
        ] {
            assert!(binaries.contains(target), "binary target missing: {target}");
        }
        assert!(binaries.contains("source_sha"));
        assert!(binaries.contains("source_tree"));
        assert!(binaries.contains("sha256sum"));
        assert!(binaries.contains("retention_days"));

        let verification = workflow_job("verify-binaries");
        assert!(verification.contains("actions: read"));
        assert!(verification.contains("jq -e"));
        assert!(verification.contains("sha256sum --check"));
    }

    #[test]
    fn workflow_fixture_keeps_nonpublishing_modes_nonmutating() {
        for job in ["release-rehearse", "build-binaries"] {
            let block = workflow_job(job);
            assert!(!block.contains("cargo publish"), "{job} must not publish");
            assert!(!block.contains("shipper publish"), "{job} must not publish");
            assert!(
                !block.contains("gh release create"),
                "{job} must not create a release"
            );
        }
        let rehearse = workflow_job("release-rehearse");
        assert!(rehearse.contains("mode == 'rehearse'"));
        let binaries = workflow_job("build-binaries");
        assert!(binaries.contains("mode == 'binaries'"));
    }

    #[test]
    fn workflow_fixture_binds_reviewed_notes_and_resume_identity() {
        let release = workflow_job("create-release");
        assert!(release.contains("generate_release_notes: false"));
        assert!(release.contains("body_path: RELEASE_NOTES_v"));
        assert!(release.contains("verify-binaries"));
        assert!(release.contains("needs.publish-crates-io.result == 'success'"));

        let publish = workflow_job("publish-crates-io");
        assert!(publish.contains("https://crates.io/api/v1/crates/"));
        assert!(publish.contains("https://index.crates.io/"));
        assert!(publish.contains("jq -s -e"));
        assert!(publish.contains("cargo install shipper --version"));

        let resume = workflow_job("release-resume");
        assert!(resume.contains("artifact_run_id"));
        assert!(resume.contains(".shipper/release-identity.json"));
        assert!(resume.contains("--artifact .shipper/release-identity.json"));
    }

    #[test]
    fn expected_package_surface_is_thirteen_crates() {
        assert_eq!(EXPECTED_PUBLISHABLE_PACKAGES.len(), 13);
        assert!(EXPECTED_PUBLISHABLE_PACKAGES.contains(&"shipper"));
        assert!(!EXPECTED_PUBLISHABLE_PACKAGES.contains(&"xtask"));
    }
}
