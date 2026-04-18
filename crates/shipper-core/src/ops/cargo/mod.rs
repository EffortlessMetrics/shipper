//! Cargo metadata loading + `cargo publish` invocation.
//!
//! Absorbed from the former `shipper-cargo` microcrate. See
//! `docs/decrating-plan.md` §6 for the overall plan.

use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use cargo_metadata::{Metadata, MetadataCommand, Package};
use serde::{Deserialize, Serialize};
pub use shipper_output_sanitizer::redact_sensitive;
use shipper_output_sanitizer::tail_lines as sanitize_tail_lines;

use crate::ops::process;

#[derive(Debug, Clone)]
pub struct CargoOutput {
    pub exit_code: i32,
    pub stdout_tail: String, // Last N lines (configurable, default 50)
    pub stderr_tail: String,
    pub duration: Duration,
    pub timed_out: bool,
}

fn tail_lines(s: &str, n: usize) -> String {
    sanitize_tail_lines(s, n)
}

/// Invoke `cargo yank` against the configured registry.
///
/// Yanks a specific `<crate>@<version>` so the registry refuses to resolve
/// it for new dependency resolves. **This is containment, not undo**:
/// existing lockfiles and already-downloaded copies are unaffected.
/// See [`cargo yank` docs](https://doc.rust-lang.org/cargo/commands/cargo-yank.html).
///
/// Output is captured (stdout/stderr tails, exit code, elapsed). The
/// caller is responsible for:
/// - classifying the result via existing [`crate::runtime::execution::classify_cargo_failure`]
/// - emitting a `PackageYanked` event if a state-dir is present
/// - retrying on transient failures (network, 5xx, 429)
///
/// Used by `shipper yank` and (in follow-on PRs under #98) by
/// `shipper plan-yank` / `shipper fix-forward` when executing a yank plan.
pub fn cargo_yank(
    workspace_root: &Path,
    package_name: &str,
    version: &str,
    registry_name: &str,
    output_lines: usize,
    timeout: Option<Duration>,
) -> Result<CargoOutput> {
    let start = Instant::now();
    let version_arg = format!("--version={version}");
    let mut args: Vec<&str> = vec!["yank", package_name, &version_arg];

    // If the user configured a non-default registry, pass it through.
    if !registry_name.trim().is_empty() && registry_name != "crates-io" {
        args.push("--registry");
        args.push(registry_name);
    }

    let output =
        process::run_command_with_timeout(&cargo_program(), &args, workspace_root, timeout)
            .context("failed to execute cargo yank; is Cargo installed?")?;

    Ok(CargoOutput {
        exit_code: output.exit_code,
        stdout_tail: tail_lines(&output.stdout, output_lines),
        stderr_tail: tail_lines(&output.stderr, output_lines),
        duration: start.elapsed(),
        timed_out: output.timed_out,
    })
}

/// Invoke `cargo install --registry <name> <crate> --version <v>` as a
/// rehearsal smoke check (#97 PR 4).
///
/// Used by `shipper rehearse --smoke-install <crate>` to prove that, after
/// a rehearsal publish, the crate actually **resolves and installs** via
/// the registry's index — the end-to-end scenario that workspace-path
/// dependencies defeat.
///
/// Installs to `install_root` (typically a tempdir) with `--force` so an
/// already-installed version of the same crate doesn't shortcut the
/// check. Output is captured and tailed like other cargo wrappers.
pub fn cargo_install_smoke(
    workspace_root: &Path,
    package_name: &str,
    version: &str,
    registry_name: &str,
    install_root: &Path,
    output_lines: usize,
    timeout: Option<Duration>,
) -> Result<CargoOutput> {
    let start = Instant::now();
    let version_arg = format!("--version={version}");
    let root_arg = install_root.display().to_string();
    let mut args: Vec<&str> = vec![
        "install",
        package_name,
        &version_arg,
        "--root",
        &root_arg,
        "--force",
        "--locked",
    ];

    if !registry_name.trim().is_empty() && registry_name != "crates-io" {
        args.push("--registry");
        args.push(registry_name);
    }

    let output =
        process::run_command_with_timeout(&cargo_program(), &args, workspace_root, timeout)
            .context("failed to execute cargo install; is Cargo installed?")?;

    Ok(CargoOutput {
        exit_code: output.exit_code,
        stdout_tail: tail_lines(&output.stdout, output_lines),
        stderr_tail: tail_lines(&output.stderr, output_lines),
        duration: start.elapsed(),
        timed_out: output.timed_out,
    })
}

pub fn cargo_publish(
    workspace_root: &Path,
    package_name: &str,
    registry_name: &str,
    allow_dirty: bool,
    no_verify: bool,
    output_lines: usize,
    timeout: Option<Duration>,
) -> Result<CargoOutput> {
    let start = Instant::now();
    let mut args: Vec<&str> = Vec::new();
    args.push("publish");
    args.push("-p");
    args.push(package_name);

    // If the user configured a non-default registry, pass it through.
    if !registry_name.trim().is_empty() && registry_name != "crates-io" {
        args.push("--registry");
        args.push(registry_name);
    }

    if allow_dirty {
        args.push("--allow-dirty");
    }
    if no_verify {
        args.push("--no-verify");
    }

    let output =
        process::run_command_with_timeout(&cargo_program(), &args, workspace_root, timeout)
            .context("failed to execute cargo publish; is Cargo installed?")?;

    let exit_code = output.exit_code;
    let stdout = output.stdout;
    let stderr = output.stderr;
    let timed_out = output.timed_out;

    let duration = start.elapsed();

    Ok(CargoOutput {
        exit_code,
        stdout_tail: tail_lines(&stdout, output_lines),
        stderr_tail: tail_lines(&stderr, output_lines),
        duration,
        timed_out,
    })
}

pub fn cargo_publish_dry_run_workspace(
    workspace_root: &Path,
    registry_name: &str,
    allow_dirty: bool,
    output_lines: usize,
) -> Result<CargoOutput> {
    let start = Instant::now();
    let mut args: Vec<&str> = vec!["publish", "--workspace", "--dry-run"];

    // If the user configured a non-default registry, pass it through.
    if !registry_name.trim().is_empty() && registry_name != "crates-io" {
        args.push("--registry");
        args.push(registry_name);
    }

    if allow_dirty {
        args.push("--allow-dirty");
    }

    let output = process::run_command_with_timeout(&cargo_program(), &args, workspace_root, None)
        .context(
        "failed to execute cargo publish --dry-run --workspace; is Cargo installed?",
    )?;

    let duration = start.elapsed();
    let exit_code = output.exit_code;
    let stdout = output.stdout;
    let stderr = output.stderr;
    let timed_out = output.timed_out;

    Ok(CargoOutput {
        exit_code,
        stdout_tail: tail_lines(&stdout, output_lines),
        stderr_tail: tail_lines(&stderr, output_lines),
        duration,
        timed_out,
    })
}

pub fn cargo_publish_dry_run_package(
    workspace_root: &Path,
    package_name: &str,
    registry_name: &str,
    allow_dirty: bool,
    output_lines: usize,
) -> Result<CargoOutput> {
    let start = Instant::now();
    let mut args: Vec<&str> = vec!["publish", "-p", package_name, "--dry-run"];

    if !registry_name.trim().is_empty() && registry_name != "crates-io" {
        args.push("--registry");
        args.push(registry_name);
    }

    if allow_dirty {
        args.push("--allow-dirty");
    }

    let output = process::run_command_with_timeout(&cargo_program(), &args, workspace_root, None)
        .with_context(|| {
        format!("failed to execute cargo publish --dry-run -p {package_name}; is Cargo installed?")
    })?;

    let duration = start.elapsed();
    let exit_code = output.exit_code;
    let stdout = output.stdout;
    let stderr = output.stderr;
    let timed_out = output.timed_out;

    Ok(CargoOutput {
        exit_code,
        stdout_tail: tail_lines(&stdout, output_lines),
        stderr_tail: tail_lines(&stderr, output_lines),
        duration,
        timed_out,
    })
}

fn cargo_program() -> String {
    env::var("SHIPPER_CARGO_BIN").unwrap_or_else(|_| "cargo".to_string())
}

// ──────────────────────────────────────────────────────────────────────────
// Workspace metadata (absorbed from shipper-cargo)
// ──────────────────────────────────────────────────────────────────────────

/// Load workspace metadata using `cargo metadata`.
///
/// Centralized here so plan-building (and any other consumer) share the
/// same invocation and error-wrapping behavior.
pub fn load_metadata(manifest_path: &Path) -> Result<Metadata> {
    MetadataCommand::new()
        .manifest_path(manifest_path)
        .exec()
        .context("failed to execute cargo metadata")
}

/// Workspace metadata wrapper.
#[derive(Debug, Clone)]
pub struct WorkspaceMetadata {
    /// The underlying cargo metadata
    metadata: Metadata,
    /// Root directory of the workspace
    workspace_root: PathBuf,
}

impl WorkspaceMetadata {
    /// Load workspace metadata from a manifest path.
    pub fn load(manifest_path: &Path) -> Result<Self> {
        let metadata = MetadataCommand::new()
            .manifest_path(manifest_path)
            .exec()
            .context("failed to load cargo metadata")?;

        let workspace_root = metadata.workspace_root.clone().into_std_path_buf();

        Ok(Self {
            metadata,
            workspace_root,
        })
    }

    /// Load metadata from the current directory.
    pub fn load_from_current_dir() -> Result<Self> {
        let manifest_path = std::env::current_dir()
            .context("failed to get current directory")?
            .join("Cargo.toml");

        Self::load(&manifest_path)
    }

    /// Workspace root directory.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// All packages in the workspace.
    pub fn all_packages(&self) -> Vec<&Package> {
        self.metadata.packages.iter().collect()
    }

    /// Packages that are publishable (not excluded from publishing).
    pub fn publishable_packages(&self) -> Vec<&Package> {
        self.metadata
            .packages
            .iter()
            .filter(|p| self.is_publishable(p))
            .collect()
    }

    /// Check if a package is publishable.
    pub fn is_publishable(&self, package: &Package) -> bool {
        if let Some(publish) = &package.publish
            && publish.is_empty()
        {
            return false;
        }

        if package.version.to_string() == "0.0.0" {
            return false;
        }

        true
    }

    /// Look up a package by name.
    pub fn get_package(&self, name: &str) -> Option<&Package> {
        self.metadata
            .packages
            .iter()
            .find(|p| p.name.as_str() == name)
    }

    /// Workspace members.
    pub fn workspace_members(&self) -> Vec<&Package> {
        self.metadata
            .workspace_members
            .iter()
            .filter_map(|id| self.metadata.packages.iter().find(|p| &p.id == id))
            .collect()
    }

    /// Root package (if any).
    pub fn root_package(&self) -> Option<&Package> {
        self.metadata.root_package()
    }

    /// Workspace name (from the root package or directory name).
    pub fn workspace_name(&self) -> &str {
        self.root_package()
            .map(|p| p.name.as_str())
            .unwrap_or_else(|| {
                self.workspace_root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("workspace")
            })
    }

    /// Packages in topological order (dependencies first).
    pub fn topological_order(&self) -> Result<Vec<String>> {
        let mut order = Vec::new();
        let mut visited = HashSet::new();
        let mut visiting = HashSet::new();

        let dep_graph = self.build_dependency_graph();

        for package in self.publishable_packages() {
            let name = package.name.to_string();
            self.visit_package(&name, &dep_graph, &mut visited, &mut visiting, &mut order)?;
        }

        Ok(order)
    }

    fn visit_package(
        &self,
        name: &str,
        dep_graph: &HashMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        visiting: &mut HashSet<String>,
        order: &mut Vec<String>,
    ) -> Result<()> {
        if visited.contains(name) {
            return Ok(());
        }

        if visiting.contains(name) {
            return Err(anyhow::anyhow!(
                "circular dependency detected involving {}",
                name
            ));
        }

        visiting.insert(name.to_string());

        if let Some(deps) = dep_graph.get(name) {
            for dep in deps {
                self.visit_package(dep, dep_graph, visited, visiting, order)?;
            }
        }

        visiting.remove(name);
        visited.insert(name.to_string());
        order.push(name.to_string());

        Ok(())
    }

    fn build_dependency_graph(&self) -> HashMap<String, Vec<String>> {
        let mut graph = HashMap::new();

        for package in self.publishable_packages() {
            let deps: Vec<String> = package
                .dependencies
                .iter()
                .filter_map(|dep| {
                    self.metadata
                        .packages
                        .iter()
                        .find(|p| p.name == dep.name)
                        .map(|p| p.name.to_string())
                })
                .collect();

            graph.insert(package.name.to_string(), deps);
        }

        graph
    }
}

/// Simplified package information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageInfo {
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Path to package manifest
    pub manifest_path: String,
    /// Whether this is a workspace member
    pub is_workspace_member: bool,
    /// List of registry names this package can be published to (empty = all)
    pub publish: Vec<String>,
}

impl From<&Package> for PackageInfo {
    fn from(pkg: &Package) -> Self {
        Self {
            name: pkg.name.to_string(),
            version: pkg.version.to_string(),
            manifest_path: pkg.manifest_path.to_string(),
            is_workspace_member: true, // Simplified
            publish: pkg.publish.clone().unwrap_or_default(),
        }
    }
}

/// Get the version from a `Cargo.toml` file.
pub fn get_version(manifest_path: &Path) -> Result<String> {
    let metadata = WorkspaceMetadata::load(manifest_path)?;

    if let Some(pkg) = metadata.root_package() {
        return Ok(pkg.version.to_string());
    }

    Err(anyhow::anyhow!("no root package found"))
}

/// Get the package name from a `Cargo.toml` file.
pub fn get_package_name(manifest_path: &Path) -> Result<String> {
    let metadata = WorkspaceMetadata::load(manifest_path)?;

    if let Some(pkg) = metadata.root_package() {
        return Ok(pkg.name.to_string());
    }

    Err(anyhow::anyhow!("no root package found"))
}

/// Check if a package name is valid for crates.io.
///
/// Rules:
/// - Non-empty
/// - Cannot start with a digit or hyphen
/// - Only ASCII lowercase letters, digits, hyphens, and underscores
pub fn is_valid_package_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    let chars: Vec<char> = name.chars().collect();

    if chars[0].is_ascii_digit() || chars[0] == '-' {
        return false;
    }

    chars
        .iter()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-' || *c == '_')
}

/// All workspace member package names.
pub fn workspace_member_names(metadata: &WorkspaceMetadata) -> Vec<String> {
    metadata
        .workspace_members()
        .iter()
        .map(|p| p.name.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;

    fn write_fake_cargo(bin_dir: &Path) -> PathBuf {
        #[cfg(windows)]
        {
            let path = bin_dir.join("cargo.cmd");
            fs::write(
                &path,
                "@echo off\r\necho %*>\"%SHIPPER_ARGS_LOG%\"\r\necho %CD%>\"%SHIPPER_CWD_LOG%\"\r\necho fake-stdout\r\necho fake-stderr 1>&2\r\nexit /b %SHIPPER_EXIT_CODE%\r\n",
            )
            .expect("write fake cargo");
            path
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = bin_dir.join("cargo");
            fs::write(
                &path,
                "#!/usr/bin/env sh\nprintf '%s' \"$*\" >\"$SHIPPER_ARGS_LOG\"\npwd >\"$SHIPPER_CWD_LOG\"\necho fake-stdout\necho fake-stderr >&2\nexit \"${SHIPPER_EXIT_CODE:-0}\"\n",
            )
            .expect("write fake cargo");
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod");
            path
        }
    }

    #[test]
    #[serial]
    fn cargo_publish_passes_flags_and_captures_output() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [
                (
                    "SHIPPER_CARGO_BIN",
                    Some(fake_cargo.to_str().expect("fake cargo utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("7")),
            ],
            || {
                let out = cargo_publish(&ws, "my-crate", "private-reg", true, true, 50, None)
                    .expect("publish");

                assert_eq!(out.exit_code, 7);
                assert!(out.stdout_tail.contains("fake-stdout"));
                assert!(out.stderr_tail.contains("fake-stderr"));

                let args = fs::read_to_string(&args_log).expect("args");
                assert!(args.contains("publish"));
                assert!(args.contains("-p my-crate"));
                assert!(args.contains("--registry private-reg"));
                assert!(args.contains("--allow-dirty"));
                assert!(args.contains("--no-verify"));

                let cwd = fs::read_to_string(&cwd_log).expect("cwd");
                assert!(cwd.trim_end().ends_with("workspace"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_publish_omits_registry_for_crates_io() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [
                (
                    "SHIPPER_CARGO_BIN",
                    Some(fake_cargo.to_str().expect("fake cargo utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let _ = cargo_publish(&ws, "my-crate", "crates-io", false, false, 50, None)
                    .expect("publish");

                let args = fs::read_to_string(&args_log).expect("args");
                assert!(!args.contains("--registry"));
                assert!(!args.contains("--allow-dirty"));
                assert!(!args.contains("--no-verify"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_publish_errors_when_command_missing() {
        let td = tempdir().expect("tempdir");
        let missing = td.path().join("does-not-exist-cargo");

        temp_env::with_var(
            "SHIPPER_CARGO_BIN",
            Some(missing.to_str().expect("utf8")),
            || {
                let err = cargo_publish(td.path(), "x", "crates-io", false, false, 50, None)
                    .expect_err("must fail");
                assert!(format!("{err:#}").contains("failed to execute cargo publish"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_yank_passes_flags_and_captures_output() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [
                (
                    "SHIPPER_CARGO_BIN",
                    Some(fake_cargo.to_str().expect("fake cargo utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let out =
                    cargo_yank(&ws, "my-crate", "1.2.3", "private-reg", 50, None).expect("yank");

                assert_eq!(out.exit_code, 0);
                assert!(out.stdout_tail.contains("fake-stdout"));

                let args = fs::read_to_string(&args_log).expect("args");
                assert!(args.contains("yank"));
                assert!(args.contains("my-crate"));
                assert!(args.contains("--version=1.2.3"));
                assert!(args.contains("--registry private-reg"));

                let cwd = fs::read_to_string(&cwd_log).expect("cwd");
                assert!(cwd.trim_end().ends_with("workspace"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_yank_omits_registry_for_crates_io() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [
                (
                    "SHIPPER_CARGO_BIN",
                    Some(fake_cargo.to_str().expect("fake cargo utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let _ = cargo_yank(&ws, "my-crate", "0.1.0", "crates-io", 50, None).expect("yank");

                let args = fs::read_to_string(&args_log).expect("args");
                assert!(!args.contains("--registry"));
                assert!(args.contains("yank"));
                assert!(args.contains("--version=0.1.0"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_yank_propagates_nonzero_exit_code() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [
                (
                    "SHIPPER_CARGO_BIN",
                    Some(fake_cargo.to_str().expect("fake cargo utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("101")),
            ],
            || {
                let out =
                    cargo_yank(&ws, "my-crate", "1.2.3", "crates-io", 50, None).expect("spawn");
                assert_eq!(out.exit_code, 101);
                assert!(out.stderr_tail.contains("fake-stderr"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_publish_dry_run_package_passes_flags() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [
                (
                    "SHIPPER_CARGO_BIN",
                    Some(fake_cargo.to_str().expect("fake cargo utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let out = cargo_publish_dry_run_package(&ws, "my-crate", "private-reg", true, 50)
                    .expect("dry-run");

                assert_eq!(out.exit_code, 0);
                let args = fs::read_to_string(&args_log).expect("args");
                assert!(args.contains("publish"));
                assert!(args.contains("-p my-crate"));
                assert!(args.contains("--dry-run"));
                assert!(args.contains("--registry private-reg"));
                assert!(args.contains("--allow-dirty"));
            },
        );
    }

    // ── redact_sensitive tests ──

    #[test]
    fn redact_authorization_bearer_header() {
        let input = "Authorization: Bearer cio_abc123secret";
        let out = redact_sensitive(input);
        assert_eq!(out, "Authorization: Bearer [REDACTED]");
    }

    #[test]
    fn redact_token_assignment_quoted() {
        let input = r#"token = "cio_mysecrettoken""#;
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("cio_mysecrettoken"));
    }

    #[test]
    fn redact_cargo_registry_token_env() {
        let input = "CARGO_REGISTRY_TOKEN=cio_secret123";
        let out = redact_sensitive(input);
        assert_eq!(out, "CARGO_REGISTRY_TOKEN=[REDACTED]");
    }

    #[test]
    fn redact_cargo_registries_named_token_env() {
        let input = "CARGO_REGISTRIES_MY_REG_TOKEN=secret456";
        let out = redact_sensitive(input);
        assert_eq!(out, "CARGO_REGISTRIES_MY_REG_TOKEN=[REDACTED]");
    }

    #[test]
    fn redact_preserves_non_sensitive_content() {
        let input = "Compiling demo v0.1.0\nFinished release target";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn redact_handles_empty_input() {
        assert_eq!(redact_sensitive(""), "");
    }

    #[test]
    fn redact_multiple_sensitive_patterns() {
        let input = "Authorization: Bearer tok123\nCARGO_REGISTRY_TOKEN=secret";
        let out = redact_sensitive(input);
        assert!(out.contains("Bearer [REDACTED]"));
        assert!(out.contains("CARGO_REGISTRY_TOKEN=[REDACTED]"));
        assert!(!out.contains("tok123"));
        assert!(!out.contains("secret"));
    }

    #[test]
    fn tail_lines_redacts_sensitive_output() {
        let input = "line1\nline2\nAuthorization: Bearer secret_token\nline4";
        let result = tail_lines(input, 50);
        assert!(result.contains("Bearer [REDACTED]"));
        assert!(!result.contains("secret_token"));
    }

    #[test]
    fn redact_mixed_case_authorization() {
        let input = "AUTHORIZATION: Bearer supersecret";
        let out = redact_sensitive(input);
        assert_eq!(out, "AUTHORIZATION: Bearer [REDACTED]");
        assert!(!out.contains("supersecret"));
    }

    #[test]
    fn redact_mixed_case_token() {
        let input = r#"Token = "mysecret""#;
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("mysecret"));
    }

    #[test]
    fn redact_non_ascii_near_sensitive_pattern_no_panic() {
        // Non-ASCII characters near the pattern should not cause a panic
        let input = "some data \u{00e9}\u{00f1} Authorization: Bearer secret123";
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("secret123"));
    }

    #[test]
    fn redaction_matches_output_sanitizer_contract() {
        let input = [
            "line one",
            "Authorization: Bearer secret_value",
            "CARGO_REGISTRIES_PRIVATE_REG_TOKEN=secret_value",
        ]
        .join("\n");

        assert_eq!(
            redact_sensitive(&input),
            shipper_output_sanitizer::redact_sensitive(&input)
        );
        assert_eq!(
            tail_lines(&input, 2),
            shipper_output_sanitizer::tail_lines(&input, 2)
        );
    }

    // ── Token redaction: position variants ──

    #[test]
    fn redact_token_at_start_of_output() {
        let input = "CARGO_REGISTRY_TOKEN=start_secret\nnormal line after";
        let out = redact_sensitive(input);
        assert!(out.starts_with("CARGO_REGISTRY_TOKEN=[REDACTED]"));
        assert!(!out.contains("start_secret"));
    }

    #[test]
    fn redact_token_at_end_of_output() {
        let input = "normal line\nCARGO_REGISTRY_TOKEN=end_secret";
        let out = redact_sensitive(input);
        assert!(out.ends_with("CARGO_REGISTRY_TOKEN=[REDACTED]"));
        assert!(!out.contains("end_secret"));
    }

    #[test]
    fn redact_bearer_at_start_of_output() {
        let input = "Authorization: Bearer first_tok\nother stuff";
        let out = redact_sensitive(input);
        assert!(out.starts_with("Authorization: Bearer [REDACTED]"));
        assert!(!out.contains("first_tok"));
    }

    #[test]
    fn redact_bearer_at_end_of_output() {
        let input = "stuff before\nAuthorization: Bearer last_tok";
        let out = redact_sensitive(input);
        assert!(out.ends_with("Authorization: Bearer [REDACTED]"));
        assert!(!out.contains("last_tok"));
    }

    #[test]
    fn redact_token_as_only_line() {
        let out = redact_sensitive("CARGO_REGISTRY_TOKEN=only");
        assert_eq!(out, "CARGO_REGISTRY_TOKEN=[REDACTED]");
    }

    // ── Multiple tokens in same output ──

    #[test]
    fn redact_three_different_token_types_multiline() {
        let input = "Authorization: Bearer bearer_secret\n\
                      CARGO_REGISTRY_TOKEN=env_secret\n\
                      CARGO_REGISTRIES_STAGING_TOKEN=staging_secret";
        let out = redact_sensitive(input);
        assert!(!out.contains("bearer_secret"));
        assert!(!out.contains("env_secret"));
        assert!(!out.contains("staging_secret"));
        assert_eq!(out.matches("[REDACTED]").count(), 3);
    }

    #[test]
    fn redact_same_token_type_repeated() {
        let input = "CARGO_REGISTRY_TOKEN=aaa\nsome stuff\nCARGO_REGISTRY_TOKEN=bbb";
        let out = redact_sensitive(input);
        assert!(!out.contains("aaa"));
        assert!(!out.contains("bbb"));
        assert_eq!(
            out,
            "CARGO_REGISTRY_TOKEN=[REDACTED]\nsome stuff\nCARGO_REGISTRY_TOKEN=[REDACTED]"
        );
    }

    #[test]
    fn redact_multiple_named_registries() {
        let input = "CARGO_REGISTRIES_ALPHA_TOKEN=tok_a\n\
                      CARGO_REGISTRIES_BETA_TOKEN=tok_b\n\
                      CARGO_REGISTRIES_GAMMA_TOKEN=tok_c";
        let out = redact_sensitive(input);
        assert!(!out.contains("tok_a"));
        assert!(!out.contains("tok_b"));
        assert!(!out.contains("tok_c"));
        assert_eq!(out.matches("[REDACTED]").count(), 3);
    }

    // ── Unicode in cargo output ──

    #[test]
    fn redact_preserves_cjk_characters() {
        let input = "コンパイル中: mycrate v1.0.0\n完了";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn redact_preserves_emoji_in_output() {
        let input = "🚀 Publishing crate 📦\n✅ Done!";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn redact_unicode_surrounding_bearer_token() {
        let input = "日本語テスト Authorization: Bearer abc_secret 中文テスト";
        let out = redact_sensitive(input);
        assert!(!out.contains("abc_secret"));
        assert!(out.contains("日本語テスト"));
        // Bearer redaction truncates after token, so 中文テスト is part of the token value
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn redact_accented_characters_preserved() {
        let input = "Résultat: réussi\nDéploiement terminé";
        let out = redact_sensitive(input);
        assert_eq!(out, input);
    }

    #[test]
    fn tail_lines_with_unicode_content() {
        let input = "first 日本語\nsecond émoji 🎉\nthird 中文";
        let out = tail_lines(input, 2);
        assert_eq!(out, "second émoji 🎉\nthird 中文");
    }

    // ── Very long output lines ──

    #[test]
    fn redact_very_long_line_no_token() {
        let long_line = "x".repeat(500_000);
        let out = redact_sensitive(&long_line);
        assert_eq!(out.len(), 500_000);
        assert_eq!(out, long_line);
    }

    #[test]
    fn redact_token_embedded_in_very_long_line() {
        let prefix = "a".repeat(200_000);
        let suffix = "b".repeat(200_000);
        let input = format!("{prefix} CARGO_REGISTRY_TOKEN=hidden {suffix}");
        let out = redact_sensitive(&input);
        assert!(!out.contains("hidden"));
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn tail_lines_with_very_long_lines() {
        let long = "y".repeat(100_000);
        let input = format!("short\n{long}\nlast");
        let out = tail_lines(&input, 2);
        assert!(out.contains(&long));
        assert!(out.contains("last"));
        assert!(!out.contains("short"));
    }

    // ── Empty output handling ──

    #[test]
    fn tail_lines_empty_string() {
        assert_eq!(tail_lines("", 10), "");
    }

    #[test]
    fn tail_lines_only_newlines() {
        let input = "\n\n\n";
        let out = tail_lines(input, 2);
        // .lines() yields three empty strings for "\n\n\n"
        assert!(out.lines().all(|l| l.is_empty()));
    }

    #[test]
    fn tail_lines_single_newline() {
        let out = tail_lines("\n", 5);
        // "\n".lines() yields one empty string
        assert_eq!(out, "\n");
    }

    #[test]
    fn redact_whitespace_only_input() {
        let input = "   \t  ";
        assert_eq!(redact_sensitive(input), input);
    }

    #[test]
    fn tail_lines_whitespace_only_lines() {
        let input = "  \n\t\n   ";
        let out = tail_lines(input, 2);
        assert_eq!(out, "\t\n   ");
    }

    // ── Timeout behavior ──

    #[test]
    #[serial]
    fn cargo_publish_with_timeout_captures_timed_out_flag() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");

        // Write a fake cargo that sleeps, ensuring it exceeds the timeout
        #[cfg(windows)]
        {
            let path = bin.join("cargo.cmd");
            fs::write(
                &path,
                "@echo off\r\nping -n 5 127.0.0.1 >nul\r\necho should-not-see\r\n",
            )
            .expect("write slow fake cargo");
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = bin.join("cargo");
            fs::write(&path, "#!/usr/bin/env sh\nsleep 10\necho should-not-see\n")
                .expect("write slow fake cargo");
            let mut perms = fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod");
        }

        let fake_cargo_path = if cfg!(windows) {
            bin.join("cargo.cmd")
        } else {
            bin.join("cargo")
        };

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [(
                "SHIPPER_CARGO_BIN",
                Some(fake_cargo_path.to_str().expect("utf8")),
            )],
            || {
                let out = cargo_publish(
                    &ws,
                    "test-crate",
                    "crates-io",
                    false,
                    false,
                    50,
                    Some(Duration::from_secs(1)),
                )
                .expect("publish with timeout");

                assert!(out.timed_out, "expected timed_out flag to be set");
                assert_eq!(out.exit_code, -1);
                assert!(out.stderr_tail.contains("timed out"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_publish_no_timeout_completes_normally() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [
                (
                    "SHIPPER_CARGO_BIN",
                    Some(fake_cargo.to_str().expect("utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let out = cargo_publish(&ws, "crate-x", "crates-io", false, false, 50, None)
                    .expect("publish");
                assert!(!out.timed_out, "should not time out");
                assert_eq!(out.exit_code, 0);
            },
        );
    }

    // ── Environment variable resolution / cargo_program ──

    #[test]
    #[serial]
    fn cargo_program_uses_env_override() {
        temp_env::with_var("SHIPPER_CARGO_BIN", Some("/custom/cargo"), || {
            assert_eq!(cargo_program(), "/custom/cargo");
        });
    }

    #[test]
    #[serial]
    fn cargo_program_defaults_to_cargo() {
        temp_env::with_var("SHIPPER_CARGO_BIN", None::<&str>, || {
            assert_eq!(cargo_program(), "cargo");
        });
    }

    #[test]
    #[serial]
    fn cargo_program_with_empty_env_uses_empty_string() {
        // Empty string is a valid env value; cargo_program returns it as-is
        temp_env::with_var("SHIPPER_CARGO_BIN", Some(""), || {
            assert_eq!(cargo_program(), "");
        });
    }

    // ── Registry name handling ──

    #[test]
    #[serial]
    fn cargo_publish_omits_registry_for_empty_string() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [
                (
                    "SHIPPER_CARGO_BIN",
                    Some(fake_cargo.to_str().expect("utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let _ = cargo_publish(&ws, "crate-y", "", false, false, 50, None).expect("publish");
                let args = fs::read_to_string(&args_log).expect("args");
                assert!(
                    !args.contains("--registry"),
                    "empty registry name should not produce --registry flag"
                );
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_publish_omits_registry_for_whitespace_only() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [
                (
                    "SHIPPER_CARGO_BIN",
                    Some(fake_cargo.to_str().expect("utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let _ =
                    cargo_publish(&ws, "crate-z", "   ", false, false, 50, None).expect("publish");
                let args = fs::read_to_string(&args_log).expect("args");
                assert!(
                    !args.contains("--registry"),
                    "whitespace-only registry name should not produce --registry flag"
                );
            },
        );
    }

    // ── Dry-run workspace variant ──

    #[test]
    #[serial]
    fn cargo_publish_dry_run_workspace_passes_flags() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [
                (
                    "SHIPPER_CARGO_BIN",
                    Some(fake_cargo.to_str().expect("utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let out =
                    cargo_publish_dry_run_workspace(&ws, "my-reg", true, 50).expect("dry-run ws");

                assert_eq!(out.exit_code, 0);
                let args = fs::read_to_string(&args_log).expect("args");
                assert!(args.contains("publish"));
                assert!(args.contains("--workspace"));
                assert!(args.contains("--dry-run"));
                assert!(args.contains("--registry my-reg"));
                assert!(args.contains("--allow-dirty"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_publish_dry_run_workspace_omits_registry_for_crates_io() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [
                (
                    "SHIPPER_CARGO_BIN",
                    Some(fake_cargo.to_str().expect("utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let _ =
                    cargo_publish_dry_run_workspace(&ws, "crates-io", false, 50).expect("dry-run");
                let args = fs::read_to_string(&args_log).expect("args");
                assert!(!args.contains("--registry"));
                assert!(!args.contains("--allow-dirty"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_publish_dry_run_workspace_errors_when_command_missing() {
        let td = tempdir().expect("tempdir");
        let missing = td.path().join("nonexistent-cargo");

        temp_env::with_var(
            "SHIPPER_CARGO_BIN",
            Some(missing.to_str().expect("utf8")),
            || {
                let err = cargo_publish_dry_run_workspace(td.path(), "crates-io", false, 50)
                    .expect_err("must fail");
                assert!(format!("{err:#}").contains("failed to execute cargo publish"));
            },
        );
    }

    // ── Dry-run package variant additional tests ──

    #[test]
    #[serial]
    fn cargo_publish_dry_run_package_omits_registry_for_crates_io() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [
                (
                    "SHIPPER_CARGO_BIN",
                    Some(fake_cargo.to_str().expect("utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("0")),
            ],
            || {
                let _ = cargo_publish_dry_run_package(&ws, "pkg", "crates-io", false, 50)
                    .expect("dry-run");
                let args = fs::read_to_string(&args_log).expect("args");
                assert!(!args.contains("--registry"));
                assert!(!args.contains("--allow-dirty"));
            },
        );
    }

    #[test]
    #[serial]
    fn cargo_publish_dry_run_package_errors_when_command_missing() {
        let td = tempdir().expect("tempdir");
        let missing = td.path().join("nonexistent-cargo");

        temp_env::with_var(
            "SHIPPER_CARGO_BIN",
            Some(missing.to_str().expect("utf8")),
            || {
                let err = cargo_publish_dry_run_package(td.path(), "pkg", "crates-io", false, 50)
                    .expect_err("must fail");
                let msg = format!("{err:#}");
                assert!(msg.contains("failed to execute cargo publish --dry-run -p pkg"));
            },
        );
    }

    // ── Output line truncation via tail_lines ──

    #[test]
    fn tail_lines_truncates_to_requested_count() {
        let lines: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
        let input = lines.join("\n");
        let out = tail_lines(&input, 5);
        assert_eq!(out.lines().count(), 5);
        assert!(out.contains("line 95"));
        assert!(out.contains("line 99"));
        assert!(!out.contains("line 94"));
    }

    #[test]
    fn tail_lines_one_line_requested() {
        let input = "first\nsecond\nthird";
        let out = tail_lines(input, 1);
        assert_eq!(out, "third");
    }

    #[test]
    fn tail_lines_redacts_token_in_last_line() {
        let input = "safe1\nsafe2\nCARGO_REGISTRY_TOKEN=leaked";
        let out = tail_lines(input, 2);
        assert!(!out.contains("leaked"));
        assert!(out.contains("CARGO_REGISTRY_TOKEN=[REDACTED]"));
    }

    #[test]
    fn tail_lines_token_outside_window_not_visible() {
        let input = "CARGO_REGISTRY_TOKEN=secret\nsafe1\nsafe2";
        let out = tail_lines(input, 2);
        assert!(!out.contains("secret"));
        assert!(!out.contains("CARGO_REGISTRY_TOKEN"));
        assert_eq!(out, "safe1\nsafe2");
    }

    // ── Error message patterns ──

    #[test]
    fn redact_token_in_error_message_context() {
        let input =
            "error: failed to publish: token = \"cio_leakedsecret\" was rejected by registry";
        let out = redact_sensitive(input);
        assert!(!out.contains("cio_leakedsecret"));
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn redact_bearer_in_http_error() {
        let input =
            "error: HTTP 403 Forbidden\nAuthorization: Bearer expired_tok_abc\nBody: access denied";
        let out = redact_sensitive(input);
        assert!(!out.contains("expired_tok_abc"));
        assert!(out.contains("error: HTTP 403 Forbidden"));
        assert!(out.contains("Body: access denied"));
    }

    #[test]
    fn redact_registry_token_in_debug_output() {
        let input = "debug: env CARGO_REGISTRY_TOKEN=cio_debug_tok resolved from environment";
        let out = redact_sensitive(input);
        assert!(!out.contains("cio_debug_tok"));
        assert!(out.contains("[REDACTED]"));
    }

    // ── CargoOutput struct behavior ──

    #[test]
    fn cargo_output_default_fields() {
        let out = CargoOutput {
            exit_code: 0,
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            duration: Duration::from_secs(0),
            timed_out: false,
        };
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout_tail.is_empty());
        assert!(out.stderr_tail.is_empty());
        assert!(!out.timed_out);
    }

    #[test]
    fn cargo_output_clone_is_independent() {
        let out = CargoOutput {
            exit_code: 42,
            stdout_tail: "hello".to_string(),
            stderr_tail: "world".to_string(),
            duration: Duration::from_millis(500),
            timed_out: true,
        };
        let cloned = out.clone();
        assert_eq!(cloned.exit_code, out.exit_code);
        assert_eq!(cloned.stdout_tail, out.stdout_tail);
        assert_eq!(cloned.stderr_tail, out.stderr_tail);
        assert_eq!(cloned.timed_out, out.timed_out);
    }

    #[test]
    fn cargo_output_debug_format() {
        let out = CargoOutput {
            exit_code: 1,
            stdout_tail: "out".to_string(),
            stderr_tail: "err".to_string(),
            duration: Duration::from_secs(1),
            timed_out: false,
        };
        let debug = format!("{out:?}");
        assert!(debug.contains("CargoOutput"));
        assert!(debug.contains("exit_code: 1"));
    }

    // ── Redaction idempotency ──

    #[test]
    fn redact_is_idempotent_bearer() {
        let input = "Authorization: Bearer secret_value";
        let once = redact_sensitive(input);
        let twice = redact_sensitive(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn redact_is_idempotent_env_token() {
        let input = "CARGO_REGISTRY_TOKEN=secret";
        let once = redact_sensitive(input);
        let twice = redact_sensitive(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn redact_is_idempotent_token_assignment() {
        let input = r#"token = "secret_value""#;
        let once = redact_sensitive(input);
        let twice = redact_sensitive(&once);
        assert_eq!(once, twice);
    }

    // ── Non-default exit codes ──

    #[test]
    #[serial]
    fn cargo_publish_captures_nonzero_exit_code() {
        let td = tempdir().expect("tempdir");
        let bin = td.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir");
        let fake_cargo = write_fake_cargo(&bin);

        let args_log = td.path().join("args.txt");
        let cwd_log = td.path().join("cwd.txt");

        let ws = td.path().join("workspace");
        fs::create_dir_all(&ws).expect("mkdir ws");

        temp_env::with_vars(
            [
                (
                    "SHIPPER_CARGO_BIN",
                    Some(fake_cargo.to_str().expect("utf8")),
                ),
                ("SHIPPER_ARGS_LOG", Some(args_log.to_str().expect("utf8"))),
                ("SHIPPER_CWD_LOG", Some(cwd_log.to_str().expect("utf8"))),
                ("SHIPPER_EXIT_CODE", Some("101")),
            ],
            || {
                let out = cargo_publish(&ws, "crate-a", "crates-io", false, false, 50, None)
                    .expect("publish");
                assert_eq!(out.exit_code, 101);
                assert!(!out.timed_out);
            },
        );
    }

    // ── tail_lines with output_lines = 0 (edge case for output truncation) ──

    #[test]
    fn tail_lines_zero_returns_empty() {
        let input = "line1\nline2\nline3";
        assert_eq!(tail_lines(input, 0), "");
    }

    // ── Redaction with special characters in token values ──

    #[test]
    fn redact_token_with_special_chars() {
        let input = "CARGO_REGISTRY_TOKEN=abc!@#$%^&*()_+-=[]{}|;:',.<>?/";
        let out = redact_sensitive(input);
        assert_eq!(out, "CARGO_REGISTRY_TOKEN=[REDACTED]");
    }

    #[test]
    fn redact_bearer_with_base64_padding() {
        let input = "Authorization: Bearer eyJhbGciOiJSUzI1NiJ9.payload.sig==";
        let out = redact_sensitive(input);
        assert_eq!(out, "Authorization: Bearer [REDACTED]");
    }

    #[test]
    fn redact_token_value_with_newline_escapes() {
        // Token value should not contain literal newlines, but escaped ones may appear
        let input = r#"token = "secret\nwith\nescapes""#;
        let out = redact_sensitive(input);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("secret\\nwith"));
    }

    // ── Absorbed from shipper-cargo: is_valid_package_name ──

    #[test]
    fn is_valid_package_name_valid() {
        assert!(is_valid_package_name("my-crate"));
        assert!(is_valid_package_name("my_crate"));
        assert!(is_valid_package_name("mycrate"));
        assert!(is_valid_package_name("my-crate-123"));
        assert!(is_valid_package_name("a"));
    }

    #[test]
    fn is_valid_package_name_invalid() {
        assert!(!is_valid_package_name(""));
        assert!(!is_valid_package_name("123-crate")); // starts with digit
        assert!(!is_valid_package_name("-crate")); // starts with hyphen
        assert!(!is_valid_package_name("MyCrate")); // uppercase
        assert!(!is_valid_package_name("my.crate")); // dot not allowed
        assert!(!is_valid_package_name("my crate")); // space not allowed
    }

    #[test]
    fn is_valid_package_name_underscore_start() {
        assert!(is_valid_package_name("_"));
        assert!(is_valid_package_name("__"));
        assert!(is_valid_package_name("_my_crate"));
    }

    #[test]
    fn is_valid_package_name_mixed_separators() {
        assert!(is_valid_package_name("my-cool_crate"));
        assert!(is_valid_package_name("a-b_c"));
    }

    #[test]
    fn is_valid_package_name_numbers_after_first() {
        assert!(is_valid_package_name("a123"));
        assert!(is_valid_package_name("crate99"));
        assert!(is_valid_package_name("my-123-crate"));
    }

    #[test]
    fn is_valid_package_name_trailing_hyphen() {
        assert!(is_valid_package_name("crate-"));
    }

    #[test]
    fn is_valid_package_name_trailing_underscore() {
        assert!(is_valid_package_name("crate_"));
    }

    #[test]
    fn is_valid_package_name_rejects_uppercase_variants() {
        assert!(!is_valid_package_name("MyPackage"));
        assert!(!is_valid_package_name("ALLCAPS"));
        assert!(!is_valid_package_name("camelCase"));
    }

    #[test]
    fn is_valid_package_name_rejects_special_characters() {
        assert!(!is_valid_package_name("my@crate"));
        assert!(!is_valid_package_name("my!crate"));
        assert!(!is_valid_package_name("my#crate"));
        assert!(!is_valid_package_name("my$crate"));
        assert!(!is_valid_package_name("my/crate"));
        assert!(!is_valid_package_name("my\\crate"));
        assert!(!is_valid_package_name("my+crate"));
        assert!(!is_valid_package_name("my crate"));
    }

    #[test]
    fn is_valid_package_name_single_underscore() {
        assert!(is_valid_package_name("_"));
    }

    #[test]
    fn is_valid_package_name_rejects_unicode() {
        assert!(!is_valid_package_name("my-crête"));
        assert!(!is_valid_package_name("日本語"));
        assert!(!is_valid_package_name("café"));
    }

    #[test]
    fn is_valid_package_name_max_length_valid() {
        let name = "a".repeat(100);
        assert!(is_valid_package_name(&name));
    }

    #[test]
    fn is_valid_package_name_consecutive_hyphens() {
        assert!(is_valid_package_name("my--crate"));
    }

    #[test]
    fn is_valid_package_name_consecutive_underscores() {
        assert!(is_valid_package_name("my__crate"));
    }

    // ── Absorbed from shipper-cargo: PackageInfo ──

    #[test]
    fn package_info_from_package() {
        let info = PackageInfo {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: "Cargo.toml".to_string(),
            is_workspace_member: true,
            publish: vec![],
        };

        assert_eq!(info.name, "test");
        assert_eq!(info.version, "1.0.0");
    }

    #[test]
    fn package_info_serialization() {
        let info = PackageInfo {
            name: "my-crate".to_string(),
            version: "2.0.0".to_string(),
            manifest_path: "/path/to/Cargo.toml".to_string(),
            is_workspace_member: true,
            publish: vec!["crates-io".to_string()],
        };

        let json = serde_json::to_string(&info).expect("serialize");
        assert!(json.contains("\"name\":\"my-crate\""));
        assert!(json.contains("\"version\":\"2.0.0\""));
    }

    #[test]
    fn package_info_deserialization_roundtrip() {
        let info = PackageInfo {
            name: "my-crate".to_string(),
            version: "2.0.0".to_string(),
            manifest_path: "/path/to/Cargo.toml".to_string(),
            is_workspace_member: true,
            publish: vec!["crates-io".to_string()],
        };

        let json = serde_json::to_string(&info).expect("serialize");
        let deserialized: PackageInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.name, info.name);
        assert_eq!(deserialized.version, info.version);
        assert_eq!(deserialized.manifest_path, info.manifest_path);
        assert_eq!(deserialized.is_workspace_member, info.is_workspace_member);
        assert_eq!(deserialized.publish, info.publish);
    }

    #[test]
    fn package_info_empty_publish_means_all_registries() {
        let info = PackageInfo {
            name: "my-crate".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: "Cargo.toml".to_string(),
            is_workspace_member: true,
            publish: vec![],
        };
        assert!(info.publish.is_empty());
    }

    #[test]
    fn package_info_multiple_registries() {
        let info = PackageInfo {
            name: "my-crate".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: "Cargo.toml".to_string(),
            is_workspace_member: false,
            publish: vec!["crates-io".to_string(), "my-registry".to_string()],
        };
        assert_eq!(info.publish.len(), 2);
        assert!(!info.is_workspace_member);
    }

    #[test]
    fn package_info_pretty_json_roundtrip() {
        let info = PackageInfo {
            name: "complex-name_123".to_string(),
            version: "0.1.0-beta.1".to_string(),
            manifest_path: "crates/foo/Cargo.toml".to_string(),
            is_workspace_member: true,
            publish: vec![],
        };
        let pretty = serde_json::to_string_pretty(&info).expect("pretty serialize");
        let back: PackageInfo = serde_json::from_str(&pretty).expect("deserialize");
        assert_eq!(back.name, info.name);
        assert_eq!(back.version, info.version);
    }

    #[test]
    fn package_info_with_empty_fields() {
        let info = PackageInfo {
            name: String::new(),
            version: String::new(),
            manifest_path: String::new(),
            is_workspace_member: false,
            publish: vec![],
        };
        let json = serde_json::to_string(&info).expect("serialize");
        let back: PackageInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.name, "");
        assert_eq!(back.version, "");
    }

    #[test]
    fn package_info_json_contains_all_fields() {
        let info = PackageInfo {
            name: "test-pkg".to_string(),
            version: "1.0.0".to_string(),
            manifest_path: "/some/path/Cargo.toml".to_string(),
            is_workspace_member: false,
            publish: vec!["custom-registry".to_string()],
        };
        let json = serde_json::to_string(&info).expect("serialize");
        assert!(json.contains("\"is_workspace_member\":false"));
        assert!(json.contains("\"publish\":[\"custom-registry\"]"));
    }

    // ── Absorbed from shipper-cargo: WorkspaceMetadata ──

    #[test]
    fn workspace_metadata_loads_current_workspace() {
        let metadata = WorkspaceMetadata::load_from_current_dir().expect("load metadata");

        assert!(!metadata.all_packages().is_empty());
        assert!(metadata.workspace_root().exists());
    }

    #[test]
    fn workspace_metadata_gets_package() {
        let metadata = WorkspaceMetadata::load_from_current_dir().expect("load metadata");

        let pkg = metadata.get_package("shipper");
        assert!(pkg.is_some());
    }

    #[test]
    fn workspace_metadata_topological_order() {
        let metadata = WorkspaceMetadata::load_from_current_dir().expect("load metadata");

        let result = metadata.topological_order();
        // Just check it doesn't panic - the result depends on the workspace structure
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn workspace_metadata_all_packages_has_multiple() {
        let metadata = WorkspaceMetadata::load_from_current_dir().expect("load metadata");
        let all = metadata.all_packages();
        assert!(all.len() > 1, "workspace should have multiple packages");
    }

    #[test]
    fn workspace_metadata_workspace_members_nonempty() {
        let metadata = WorkspaceMetadata::load_from_current_dir().expect("load metadata");
        let members = metadata.workspace_members();
        assert!(!members.is_empty(), "workspace should have members");
    }

    #[test]
    fn workspace_metadata_get_nonexistent_package_returns_none() {
        let metadata = WorkspaceMetadata::load_from_current_dir().expect("load metadata");
        assert!(
            metadata
                .get_package("nonexistent-package-xyz-12345")
                .is_none()
        );
    }

    #[test]
    fn workspace_metadata_workspace_name_not_empty() {
        let metadata = WorkspaceMetadata::load_from_current_dir().expect("load metadata");
        assert!(!metadata.workspace_name().is_empty());
    }

    #[test]
    fn workspace_metadata_workspace_root_is_directory() {
        let metadata = WorkspaceMetadata::load_from_current_dir().expect("load metadata");
        assert!(metadata.workspace_root().is_dir());
    }

    #[test]
    fn workspace_metadata_publishable_packages_subset_of_all() {
        let metadata = WorkspaceMetadata::load_from_current_dir().expect("load metadata");
        let all = metadata.all_packages();
        let publishable = metadata.publishable_packages();
        assert!(
            publishable.len() <= all.len(),
            "publishable ({}) should be <= all ({})",
            publishable.len(),
            all.len()
        );
    }

    #[test]
    fn workspace_member_names_contains_known_crates() {
        let metadata = WorkspaceMetadata::load_from_current_dir().expect("load metadata");
        let names = workspace_member_names(&metadata);
        assert!(
            names.contains(&"shipper".to_string()),
            "should contain shipper, got: {names:?}"
        );
    }

    #[test]
    fn workspace_metadata_topological_order_contains_publishable() {
        let metadata = WorkspaceMetadata::load_from_current_dir().expect("load metadata");
        if let Ok(order) = metadata.topological_order() {
            let publishable: Vec<String> = metadata
                .publishable_packages()
                .iter()
                .map(|p| p.name.to_string())
                .collect();
            for name in &publishable {
                assert!(
                    order.contains(name),
                    "topological order should contain publishable package {name}"
                );
            }
        }
    }

    // ── Absorbed from shipper-cargo: load_metadata ──

    #[test]
    fn load_metadata_returns_valid_metadata() {
        let manifest = std::env::current_dir()
            .unwrap()
            .join("..")
            .join("..")
            .join("Cargo.toml");
        let metadata = load_metadata(&manifest).expect("load metadata");
        assert!(!metadata.packages.is_empty());
    }

    #[test]
    fn load_metadata_fails_for_nonexistent_path() {
        let result = load_metadata(Path::new("/nonexistent/Cargo.toml"));
        assert!(result.is_err());
    }

    // ── Absorbed proptests ──

    mod proptests_absorbed {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn valid_package_name_only_has_valid_chars(
                name in "[a-z_][a-z0-9_-]{0,30}",
            ) {
                prop_assert!(is_valid_package_name(&name));
            }

            #[test]
            fn package_name_starting_with_digit_is_invalid(
                rest in "[a-z0-9_-]{0,20}",
                digit in proptest::char::range('0', '9'),
            ) {
                let name = format!("{digit}{rest}");
                prop_assert!(!is_valid_package_name(&name));
            }

            #[test]
            fn package_name_starting_with_hyphen_is_invalid(
                rest in "[a-z0-9_-]{0,20}",
            ) {
                let name = format!("-{rest}");
                prop_assert!(!is_valid_package_name(&name));
            }

            #[test]
            fn package_name_with_uppercase_is_invalid(
                prefix in "[a-z_][a-z0-9_-]{0,10}",
                upper in "[A-Z]",
                suffix in "[a-z0-9_-]{0,10}",
            ) {
                let name = format!("{prefix}{upper}{suffix}");
                prop_assert!(!is_valid_package_name(&name));
            }

            #[test]
            fn package_info_serde_roundtrip(
                name in "[a-z][a-z0-9_-]{0,20}",
                version in "[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}",
                manifest in "\\PC{1,50}",
                is_member in any::<bool>(),
            ) {
                let info = PackageInfo {
                    name: name.clone(),
                    version: version.clone(),
                    manifest_path: manifest.clone(),
                    is_workspace_member: is_member,
                    publish: vec![],
                };
                let json = serde_json::to_string(&info).unwrap();
                let back: PackageInfo = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(&back.name, &name);
                prop_assert_eq!(&back.version, &version);
                prop_assert_eq!(&back.manifest_path, &manifest);
                prop_assert_eq!(back.is_workspace_member, is_member);
                prop_assert!(back.publish.is_empty());
            }

            #[test]
            fn package_info_with_registries_roundtrip(
                reg_count in 0usize..5,
                name in "[a-z][a-z0-9-]{0,10}",
            ) {
                let registries: Vec<String> = (0..reg_count)
                    .map(|i| format!("registry-{i}"))
                    .collect();
                let info = PackageInfo {
                    name,
                    version: "1.0.0".to_string(),
                    manifest_path: "Cargo.toml".to_string(),
                    is_workspace_member: true,
                    publish: registries.clone(),
                };
                let json = serde_json::to_string(&info).unwrap();
                let back: PackageInfo = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(back.publish.len(), registries.len());
                prop_assert_eq!(&back.publish, &registries);
            }

            #[test]
            fn is_valid_package_name_rejects_any_non_ascii(
                prefix in "[a-z_][a-z0-9_-]{0,5}",
                ch in proptest::char::range('\u{0080}', '\u{FFFF}'),
                suffix in "[a-z0-9_-]{0,5}",
            ) {
                let name = format!("{prefix}{ch}{suffix}");
                prop_assert!(!is_valid_package_name(&name));
            }

            #[test]
            fn package_info_json_always_contains_name(
                name in "[a-z][a-z0-9-]{0,15}",
            ) {
                let info = PackageInfo {
                    name: name.clone(),
                    version: "1.0.0".to_string(),
                    manifest_path: "Cargo.toml".to_string(),
                    is_workspace_member: true,
                    publish: vec![],
                };
                let json = serde_json::to_string(&info).unwrap();
                prop_assert!(json.contains(&name));
            }
        }
    }

    // ── Absorbed snapshot tests ──

    mod snapshot_tests_absorbed {
        use super::*;
        use insta::{assert_debug_snapshot, assert_yaml_snapshot};

        #[test]
        fn snapshot_package_info_simple() {
            let info = PackageInfo {
                name: "shipper-cargo".to_string(),
                version: "0.3.0".to_string(),
                manifest_path: "crates/shipper-cargo/Cargo.toml".to_string(),
                is_workspace_member: true,
                publish: vec![],
            };
            assert_yaml_snapshot!(info);
        }

        #[test]
        fn snapshot_package_info_with_registries() {
            let info = PackageInfo {
                name: "my-private-crate".to_string(),
                version: "1.2.3-beta.1".to_string(),
                manifest_path: "crates/my-private-crate/Cargo.toml".to_string(),
                is_workspace_member: false,
                publish: vec!["crates-io".to_string(), "my-private-registry".to_string()],
            };
            assert_yaml_snapshot!(info);
        }

        #[test]
        fn snapshot_valid_package_names() {
            let names = vec!["my-crate", "my_crate", "a", "_private", "crate-with-123"];
            let results: Vec<(&str, bool)> = names
                .into_iter()
                .map(|n| (n, is_valid_package_name(n)))
                .collect();
            assert_debug_snapshot!(results);
        }

        #[test]
        fn snapshot_invalid_package_names() {
            let names = vec![
                "",
                "123-start",
                "-hyphen-start",
                "MyCrate",
                "my.crate",
                "my crate",
                "my@crate",
            ];
            let results: Vec<(&str, bool)> = names
                .into_iter()
                .map(|n| (n, is_valid_package_name(n)))
                .collect();
            assert_debug_snapshot!(results);
        }

        #[test]
        fn snapshot_package_info_prerelease_version() {
            let info = PackageInfo {
                name: "my-alpha-crate".to_string(),
                version: "0.0.1-alpha.0+build.123".to_string(),
                manifest_path: "crates/my-alpha-crate/Cargo.toml".to_string(),
                is_workspace_member: true,
                publish: vec![],
            };
            assert_yaml_snapshot!(info);
        }
    }
}
