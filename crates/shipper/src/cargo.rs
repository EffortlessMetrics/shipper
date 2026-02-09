use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct CargoOutput {
    pub status_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub fn cargo_publish(
    workspace_root: &Path,
    package_name: &str,
    registry_name: &str,
    allow_dirty: bool,
    no_verify: bool,
) -> Result<CargoOutput> {
    let mut cmd = Command::new("cargo");
    cmd.arg("publish").arg("-p").arg(package_name);

    // If the user configured a non-default registry, pass it through.
    if !registry_name.trim().is_empty() && registry_name != "crates-io" {
        cmd.arg("--registry").arg(registry_name);
    }

    if allow_dirty {
        cmd.arg("--allow-dirty");
    }
    if no_verify {
        cmd.arg("--no-verify");
    }

    let out = cmd
        .current_dir(workspace_root)
        .output()
        .context("failed to execute cargo publish; is Cargo installed?")?;

    Ok(CargoOutput {
        status_code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    })
}
