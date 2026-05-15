//! `cargo xtask badges` — generated public Shields endpoint JSON.
//!
//! Public README badges are repository-scoped trust markers. Diff-scoped RIPR
//! evidence belongs under `target/ripr/` and PR artifacts, not in committed
//! badge endpoints.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const BADGE_ENDPOINT_DIR: &str = "badges";
const BADGE_ENDPOINT_TARGET_DIR: &str = "target/xtask/badges";
const RIPR_PLUS_ENDPOINT: &str = "ripr-plus.json";

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Regenerate into target/xtask/badges and fail if committed endpoints drift.
    #[arg(long)]
    pub check: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct ShieldsEndpointBadge {
    #[serde(rename = "schemaVersion")]
    schema_version: u8,
    label: String,
    message: String,
    color: String,
}

pub fn badges(args: &Args) -> Result<()> {
    let workspace_root = workspace_root()?;
    let target_dir = workspace_root.join(BADGE_ENDPOINT_TARGET_DIR);
    fs::create_dir_all(&target_dir)
        .with_context(|| format!("creating {}", target_dir.display()))?;

    let ripr_plus = ripr_plus_badge(&workspace_root)?;
    validate_shields_badge(&ripr_plus, Some("ripr+"))?;
    write_json_pretty(&target_dir.join(RIPR_PLUS_ENDPOINT), &ripr_plus)?;

    if args.check {
        compare_files(
            &workspace_root
                .join(BADGE_ENDPOINT_DIR)
                .join(RIPR_PLUS_ENDPOINT),
            &target_dir.join(RIPR_PLUS_ENDPOINT),
        )?;
        println!("badges: committed endpoints are current");
        return Ok(());
    }

    let committed_dir = workspace_root.join(BADGE_ENDPOINT_DIR);
    fs::create_dir_all(&committed_dir)
        .with_context(|| format!("creating {}", committed_dir.display()))?;
    fs::copy(
        target_dir.join(RIPR_PLUS_ENDPOINT),
        committed_dir.join(RIPR_PLUS_ENDPOINT),
    )
    .with_context(|| format!("copying generated {RIPR_PLUS_ENDPOINT} into badges/"))?;

    println!("badges: refreshed public endpoint JSON under badges/");
    Ok(())
}

fn ripr_plus_badge(workspace_root: &Path) -> Result<ShieldsEndpointBadge> {
    let ripr_bin = std::env::var("RIPR_BIN").unwrap_or_else(|_| "ripr".to_string());

    let output = Command::new(&ripr_bin)
        .arg("check")
        .arg("--root")
        .arg(workspace_root)
        .arg("--format")
        .arg("repo-badge-plus-shields")
        .current_dir(workspace_root)
        .output()
        .with_context(|| format!("spawning `{ripr_bin} check --format repo-badge-plus-shields`"))?;

    if output.status.success() {
        return serde_json::from_slice(&output.stdout)
            .with_context(|| format!("{ripr_bin} emitted invalid Shields endpoint JSON"));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.contains("test-efficiency") {
        bail!(
            "{ripr_bin} repo-badge-plus-shields failed: {}",
            stderr.trim()
        );
    }

    ripr_plus_badge_from_repo_exposure(workspace_root, &ripr_bin)
}

fn ripr_plus_badge_from_repo_exposure(
    workspace_root: &Path,
    ripr_bin: &str,
) -> Result<ShieldsEndpointBadge> {
    let output = Command::new(ripr_bin)
        .arg("check")
        .arg("--root")
        .arg(workspace_root)
        .arg("--mode")
        .arg("ready")
        .arg("--format")
        .arg("repo-exposure-json")
        .current_dir(workspace_root)
        .output()
        .with_context(|| format!("spawning `{ripr_bin} check --format repo-exposure-json`"))?;

    if !output.status.success() {
        bail!(
            "{ripr_bin} repo-exposure-json fallback failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("{ripr_bin} emitted invalid repo-exposure JSON"))?;
    let headline = value
        .get("metrics")
        .and_then(|m| m.get("headline_eligible"))
        .and_then(serde_json::Value::as_u64)
        .context("`metrics.headline_eligible` missing from ripr repo-exposure JSON")?;

    Ok(ShieldsEndpointBadge {
        schema_version: 1,
        label: "ripr+".to_string(),
        message: headline.to_string(),
        color: shields_color(headline).to_string(),
    })
}

fn shields_color(count: u64) -> &'static str {
    match count {
        0 => "brightgreen",
        1..=99 => "yellowgreen",
        100..=999 => "orange",
        _ => "red",
    }
}

fn validate_shields_badge(
    badge: &ShieldsEndpointBadge,
    expected_label: Option<&str>,
) -> Result<()> {
    if badge.schema_version != 1 {
        bail!("badge `{}` has unsupported schemaVersion", badge.label);
    }

    if let Some(expected_label) = expected_label
        && badge.label != expected_label
    {
        bail!(
            "badge label drifted: got `{}`, expected `{expected_label}`",
            badge.label
        );
    }

    if badge.message.trim().is_empty() {
        bail!("badge `{}` has empty message", badge.label);
    }

    if badge.color.trim().is_empty() {
        bail!("badge `{}` has empty color", badge.label);
    }

    Ok(())
}

fn write_json_pretty(path: &Path, badge: &ShieldsEndpointBadge) -> Result<()> {
    let mut json = serde_json::to_string_pretty(badge)
        .with_context(|| format!("serializing {}", path.display()))?;
    json.push('\n');
    fs::write(path, json).with_context(|| format!("writing {}", path.display()))
}

fn compare_files(committed: &Path, generated: &Path) -> Result<()> {
    let committed_bytes = fs::read(committed)
        .with_context(|| format!("reading committed badge endpoint {}", committed.display()))?;
    let generated_bytes = fs::read(generated)
        .with_context(|| format!("reading generated badge endpoint {}", generated.display()))?;
    if committed_bytes != generated_bytes {
        bail!(
            "badge endpoint drift: {} differs from generated {}; run `cargo xtask badges`",
            committed.display(),
            generated.display()
        );
    }
    Ok(())
}

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .context("CARGO_MANIFEST_DIR not set; run via `cargo xtask`")?;
    let xtask_dir = PathBuf::from(manifest_dir);
    xtask_dir
        .parent()
        .with_context(|| format!("xtask manifest dir has no parent: {}", xtask_dir.display()))
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ripr_plus_badge_shape_is_stable() {
        let badge = ShieldsEndpointBadge {
            schema_version: 1,
            label: "ripr+".to_string(),
            message: "0".to_string(),
            color: "brightgreen".to_string(),
        };

        validate_shields_badge(&badge, Some("ripr+")).unwrap();
    }

    #[test]
    fn badge_rejects_wrong_label() {
        let badge = ShieldsEndpointBadge {
            schema_version: 1,
            label: "ripr".to_string(),
            message: "0".to_string(),
            color: "brightgreen".to_string(),
        };

        assert!(validate_shields_badge(&badge, Some("ripr+")).is_err());
    }

    #[test]
    fn badge_rejects_empty_message() {
        let badge = ShieldsEndpointBadge {
            schema_version: 1,
            label: "ripr+".to_string(),
            message: " ".to_string(),
            color: "brightgreen".to_string(),
        };

        assert!(validate_shields_badge(&badge, Some("ripr+")).is_err());
    }
}
