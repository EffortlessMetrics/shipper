//! Convenience wrappers for shelling out to `cargo`.

use anyhow::Result;

use super::run::{run_command, run_command_in_dir};
use super::types::CommandResult;

/// Run cargo with arguments.
#[allow(dead_code)]
pub(crate) fn run_cargo(args: &[&str]) -> Result<CommandResult> {
    run_command("cargo", args)
}

/// Run cargo in a specific directory.
#[allow(dead_code)]
pub(crate) fn run_cargo_in_dir(args: &[&str], dir: &std::path::Path) -> Result<CommandResult> {
    run_command_in_dir("cargo", args, dir)
}

/// Run cargo publish (dry run).
#[allow(dead_code)]
pub(crate) fn cargo_dry_run(manifest_path: &std::path::Path) -> Result<CommandResult> {
    run_cargo_in_dir(
        &[
            "publish",
            "--dry-run",
            "--manifest-path",
            manifest_path.to_str().unwrap_or(""),
        ],
        manifest_path.parent().unwrap_or(std::path::Path::new(".")),
    )
}

/// Run cargo publish.
#[allow(dead_code)]
pub(crate) fn cargo_publish(
    manifest_path: &std::path::Path,
    registry: Option<&str>,
) -> Result<CommandResult> {
    let mut args = vec![
        "publish",
        "--manifest-path",
        manifest_path.to_str().unwrap_or(""),
    ];

    if let Some(reg) = registry {
        args.push("--registry");
        args.push(reg);
    }

    run_cargo_in_dir(
        &args,
        manifest_path.parent().unwrap_or(std::path::Path::new(".")),
    )
}
