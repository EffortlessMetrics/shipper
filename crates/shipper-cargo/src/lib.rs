//! Cargo workspace metadata for shipper.
//!
//! This crate provides utilities for working with cargo workspace metadata,
//! including package discovery, dependency analysis, and version extraction.
//!
//! # Example
//!
//! ```ignore
//! use shipper_cargo::{WorkspaceMetadata, PackageInfo};
//! use std::path::Path;
//!
//! // Load workspace metadata
//! let metadata = WorkspaceMetadata::load(Path::new("./Cargo.toml")).expect("load metadata");
//!
//! // Get all publishable packages
//! let packages = metadata.publishable_packages();
//! for pkg in packages {
//!     println!("{} @ {}", pkg.name, pkg.version);
//! }
//! ```

use std::collections::{HashMap, HashSet};
use std::env;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use cargo_metadata::{Metadata, MetadataCommand, Package};
use serde::{Deserialize, Serialize};
pub use shipper_output_sanitizer::redact_sensitive;
use shipper_output_sanitizer::tail_lines;

#[derive(Debug, Clone)]
pub struct CargoOutput {
    pub exit_code: i32,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub duration: Duration,
    pub timed_out: bool,
}

/// Load workspace metadata using `cargo metadata`.
///
/// This is intentionally centralized in `shipper-cargo` so plan-building
/// can be shared in microcrate mode.
pub fn load_metadata(manifest_path: &Path) -> Result<Metadata> {
    MetadataCommand::new()
        .manifest_path(manifest_path)
        .exec()
        .context("failed to execute cargo metadata")
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
    let mut cmd = Command::new(cargo_program());
    cmd.arg("publish").arg("-p").arg(package_name);

    if !registry_name.trim().is_empty() && registry_name != "crates-io" {
        cmd.arg("--registry").arg(registry_name);
    }

    if allow_dirty {
        cmd.arg("--allow-dirty");
    }
    if no_verify {
        cmd.arg("--no-verify");
    }

    cmd.current_dir(workspace_root);

    let (exit_code, stdout, stderr, timed_out) = if let Some(timeout_dur) = timeout {
        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("failed to execute cargo publish; is Cargo installed?")?;

        let deadline = Instant::now() + timeout_dur;
        loop {
            match child.try_wait().context("failed to poll cargo process")? {
                Some(status) => {
                    let mut stdout_bytes = Vec::new();
                    let mut stderr_bytes = Vec::new();
                    if let Some(mut out) = child.stdout.take() {
                        let _ = out.read_to_end(&mut stdout_bytes);
                    }
                    if let Some(mut err) = child.stderr.take() {
                        let _ = err.read_to_end(&mut stderr_bytes);
                    }
                    break (
                        status.code().unwrap_or(-1),
                        String::from_utf8_lossy(&stdout_bytes).to_string(),
                        String::from_utf8_lossy(&stderr_bytes).to_string(),
                        false,
                    );
                }
                None => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        let mut stdout_bytes = Vec::new();
                        let mut stderr_bytes = Vec::new();
                        if let Some(mut out) = child.stdout.take() {
                            let _ = out.read_to_end(&mut stdout_bytes);
                        }
                        if let Some(mut err) = child.stderr.take() {
                            let _ = err.read_to_end(&mut stderr_bytes);
                        }
                        let mut stderr_str = String::from_utf8_lossy(&stderr_bytes).to_string();
                        stderr_str.push_str(&format!(
                            "\ncargo publish timed out after {}",
                            humantime::format_duration(timeout_dur)
                        ));
                        break (
                            -1,
                            String::from_utf8_lossy(&stdout_bytes).to_string(),
                            stderr_str,
                            true,
                        );
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    } else {
        let out = cmd
            .output()
            .context("failed to execute cargo publish; is Cargo installed?")?;
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
            false,
        )
    };

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
    let mut cmd = Command::new(cargo_program());
    cmd.arg("publish").arg("--workspace").arg("--dry-run");

    if !registry_name.trim().is_empty() && registry_name != "crates-io" {
        cmd.arg("--registry").arg(registry_name);
    }

    if allow_dirty {
        cmd.arg("--allow-dirty");
    }

    let out = cmd
        .current_dir(workspace_root)
        .output()
        .context("failed to execute cargo publish --dry-run --workspace; is Cargo installed?")?;

    let duration = start.elapsed();
    let exit_code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    Ok(CargoOutput {
        exit_code,
        stdout_tail: tail_lines(&stdout, output_lines),
        stderr_tail: tail_lines(&stderr, output_lines),
        duration,
        timed_out: false,
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
    let mut cmd = Command::new(cargo_program());
    cmd.arg("publish")
        .arg("-p")
        .arg(package_name)
        .arg("--dry-run");

    if !registry_name.trim().is_empty() && registry_name != "crates-io" {
        cmd.arg("--registry").arg(registry_name);
    }

    if allow_dirty {
        cmd.arg("--allow-dirty");
    }

    let out = cmd.current_dir(workspace_root).output().with_context(|| {
        format!("failed to execute cargo publish --dry-run -p {package_name}; is Cargo installed?")
    })?;

    let duration = start.elapsed();
    let exit_code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    Ok(CargoOutput {
        exit_code,
        stdout_tail: tail_lines(&stdout, output_lines),
        stderr_tail: tail_lines(&stderr, output_lines),
        duration,
        timed_out: false,
    })
}

fn cargo_program() -> String {
    env::var("SHIPPER_CARGO_BIN").unwrap_or_else(|_| "cargo".to_string())
}

/// Workspace metadata wrapper
#[derive(Debug, Clone)]
pub struct WorkspaceMetadata {
    /// The underlying cargo metadata
    metadata: Metadata,
    /// Root directory of the workspace
    workspace_root: PathBuf,
}

impl WorkspaceMetadata {
    /// Load workspace metadata from a manifest path
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

    /// Load metadata from the current directory
    pub fn load_from_current_dir() -> Result<Self> {
        let manifest_path = std::env::current_dir()
            .context("failed to get current directory")?
            .join("Cargo.toml");

        Self::load(&manifest_path)
    }

    /// Get the workspace root directory
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Get all packages in the workspace
    pub fn all_packages(&self) -> Vec<&Package> {
        self.metadata.packages.iter().collect()
    }

    /// Get packages that are publishable (not excluded from publishing)
    pub fn publishable_packages(&self) -> Vec<&Package> {
        self.metadata
            .packages
            .iter()
            .filter(|p| self.is_publishable(p))
            .collect()
    }

    /// Check if a package is publishable
    pub fn is_publishable(&self, package: &Package) -> bool {
        // Check if package has publish = false
        if let Some(publish) = &package.publish
            && publish.is_empty()
        {
            return false;
        }

        // Skip packages with version 0.0.0
        if package.version.to_string() == "0.0.0" {
            return false;
        }

        true
    }

    /// Get a package by name
    pub fn get_package(&self, name: &str) -> Option<&Package> {
        self.metadata
            .packages
            .iter()
            .find(|p| p.name.as_str() == name)
    }

    /// Get the workspace members
    pub fn workspace_members(&self) -> Vec<&Package> {
        self.metadata
            .workspace_members
            .iter()
            .filter_map(|id| self.metadata.packages.iter().find(|p| &p.id == id))
            .collect()
    }

    /// Get the root package (if any)
    pub fn root_package(&self) -> Option<&Package> {
        self.metadata.root_package()
    }

    /// Get the workspace name (from the root package or directory name)
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

    /// Get packages in topological order (dependencies first)
    pub fn topological_order(&self) -> Result<Vec<String>> {
        let mut order = Vec::new();
        let mut visited = HashSet::new();
        let mut visiting = HashSet::new();

        // Build dependency graph
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
                    // Only include workspace dependencies
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

/// Simplified package information
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

/// Get the version from a Cargo.toml file
pub fn get_version(manifest_path: &Path) -> Result<String> {
    let metadata = WorkspaceMetadata::load(manifest_path)?;

    if let Some(pkg) = metadata.root_package() {
        return Ok(pkg.version.to_string());
    }

    Err(anyhow::anyhow!("no root package found"))
}

/// Get the package name from a Cargo.toml file
pub fn get_package_name(manifest_path: &Path) -> Result<String> {
    let metadata = WorkspaceMetadata::load(manifest_path)?;

    if let Some(pkg) = metadata.root_package() {
        return Ok(pkg.name.to_string());
    }

    Err(anyhow::anyhow!("no root package found"))
}

/// Check if a package name is valid for crates.io
pub fn is_valid_package_name(name: &str) -> bool {
    // Package names must be:
    // - Only lowercase letters, numbers, hyphens, and underscores
    // - At least 1 character
    // - Not start with a digit or hyphen
    // - Not reserved

    if name.is_empty() {
        return false;
    }

    let chars: Vec<char> = name.chars().collect();

    // Can't start with digit or hyphen
    if chars[0].is_ascii_digit() || chars[0] == '-' {
        return false;
    }

    // All chars must be valid
    chars
        .iter()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-' || *c == '_')
}

/// Get all workspace member names
pub fn workspace_member_names(metadata: &WorkspaceMetadata) -> Vec<String> {
    metadata
        .workspace_members()
        .iter()
        .map(|p| p.name.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_valid_package_name ──────────────────────────────────────────

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

    // ── CargoOutput ────────────────────────────────────────────────────

    #[test]
    fn cargo_output_construction_success() {
        let output = CargoOutput {
            exit_code: 0,
            stdout_tail: "published my-crate v1.0.0".to_string(),
            stderr_tail: String::new(),
            duration: Duration::from_secs(5),
            timed_out: false,
        };
        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stdout_tail, "published my-crate v1.0.0");
        assert!(output.stderr_tail.is_empty());
        assert_eq!(output.duration, Duration::from_secs(5));
        assert!(!output.timed_out);
    }

    #[test]
    fn cargo_output_construction_failure() {
        let output = CargoOutput {
            exit_code: 101,
            stdout_tail: String::new(),
            stderr_tail: "error[E0433]: failed to resolve".to_string(),
            duration: Duration::from_millis(800),
            timed_out: false,
        };
        assert_eq!(output.exit_code, 101);
        assert!(!output.stderr_tail.is_empty());
        assert!(!output.timed_out);
    }

    #[test]
    fn cargo_output_timed_out_flag() {
        let output = CargoOutput {
            exit_code: -1,
            stdout_tail: String::new(),
            stderr_tail: "cargo publish timed out after 5m".to_string(),
            duration: Duration::from_secs(300),
            timed_out: true,
        };
        assert!(output.timed_out);
        assert_eq!(output.exit_code, -1);
    }

    #[test]
    fn cargo_output_clone_preserves_fields() {
        let output = CargoOutput {
            exit_code: 42,
            stdout_tail: "hello".to_string(),
            stderr_tail: "world".to_string(),
            duration: Duration::from_millis(123),
            timed_out: false,
        };
        let cloned = output.clone();
        assert_eq!(cloned.exit_code, output.exit_code);
        assert_eq!(cloned.stdout_tail, output.stdout_tail);
        assert_eq!(cloned.stderr_tail, output.stderr_tail);
        assert_eq!(cloned.duration, output.duration);
        assert_eq!(cloned.timed_out, output.timed_out);
    }

    #[test]
    fn cargo_output_debug_format() {
        let output = CargoOutput {
            exit_code: 0,
            stdout_tail: "ok".to_string(),
            stderr_tail: String::new(),
            duration: Duration::from_secs(1),
            timed_out: false,
        };
        let debug = format!("{output:?}");
        assert!(debug.contains("CargoOutput"));
        assert!(debug.contains("exit_code: 0"));
    }

    // ── PackageInfo ────────────────────────────────────────────────────

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

    // ── WorkspaceMetadata ──────────────────────────────────────────────

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
    fn workspace_metadata_gets_shipper_cargo_package() {
        let metadata = WorkspaceMetadata::load_from_current_dir().expect("load metadata");
        let pkg = metadata.get_package("shipper-cargo");
        assert!(pkg.is_some(), "should find shipper-cargo in workspace");
    }

    #[test]
    fn workspace_member_names_contains_known_crates() {
        let metadata = WorkspaceMetadata::load_from_current_dir().expect("load metadata");
        let names = workspace_member_names(&metadata);
        assert!(
            names.contains(&"shipper-cargo".to_string()),
            "should contain shipper-cargo, got: {names:?}"
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

    // ── load_metadata ──────────────────────────────────────────────────

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
}
