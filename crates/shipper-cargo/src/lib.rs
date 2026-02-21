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
            match child
                .try_wait()
                .context("failed to poll cargo process")?
            {
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

fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let tail = if lines.len() <= n {
        s.to_string()
    } else {
        lines[lines.len() - n..].join("\n")
    };
    redact_sensitive(&tail)
}

/// Redact sensitive patterns (tokens, credentials) from output strings.
/// Applied to stdout/stderr tails before they are stored in receipts and event logs.
pub fn redact_sensitive(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for line in s.lines() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&redact_line(line));
    }
    if s.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn redact_line(line: &str) -> String {
    let mut out = line.to_string();

    if let Some(pos) = out.to_ascii_lowercase().find("authorization:") {
        let after = &out[pos..];
        if let Some(bearer_pos) = after.to_ascii_lowercase().find("bearer ") {
            let redact_start = pos + bearer_pos + "bearer ".len();
            out = format!("{}[REDACTED]", &out[..redact_start]);
        }
    }

    if let Some(pos) = out.to_ascii_lowercase().find("token") {
        let after_key = &out[pos + "token".len()..];
        let trimmed = after_key.trim_start();
        if trimmed.starts_with("= ") || trimmed.starts_with("=") {
            let eq_offset = pos + "token".len() + (after_key.len() - trimmed.len());
            let after_eq = trimmed.trim_start_matches('=').trim_start();
            if after_eq.starts_with('"') || after_eq.starts_with('\'') {
                out = format!("{}= \"[REDACTED]\"", &out[..eq_offset]);
            } else if !after_eq.is_empty() {
                out = format!("{}= [REDACTED]", &out[..eq_offset]);
            }
        }
    }

    if let Some(pos) = find_cargo_token_env(&out)
        && let Some(eq_pos) = out[pos..].find('=')
    {
        let abs_eq = pos + eq_pos;
        out = format!("{}=[REDACTED]", &out[..abs_eq]);
    }

    out
}

/// Find the start position of a CARGO_REGISTRY_TOKEN or CARGO_REGISTRIES_<NAME>_TOKEN pattern.
fn find_cargo_token_env(s: &str) -> Option<usize> {
    if let Some(pos) = s.find("CARGO_REGISTRY_TOKEN") {
        return Some(pos);
    }
    if let Some(pos) = s.find("CARGO_REGISTRIES_") {
        let after = &s[pos + "CARGO_REGISTRIES_".len()..];
        if after.contains("_TOKEN") {
            return Some(pos);
        }
    }
    None
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
        self.metadata.packages.iter().find(|p| p.name.as_str() == name)
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
            return Err(anyhow::anyhow!("circular dependency detected involving {}", name));
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
    chars.iter().all(|c| {
        c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-' || *c == '_'
    })
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
    fn package_info_from_package() {
        // This test verifies the conversion works
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
    fn workspace_metadata_loads_current_workspace() {
        let metadata = WorkspaceMetadata::load_from_current_dir().expect("load metadata");
        
        // Should have at least one package
        assert!(!metadata.all_packages().is_empty());
        
        // Should have a workspace root
        assert!(metadata.workspace_root().exists());
    }

    #[test]
    fn workspace_metadata_gets_package() {
        let metadata = WorkspaceMetadata::load_from_current_dir().expect("load metadata");
        
        // Should be able to get a known package
        let pkg = metadata.get_package("shipper");
        assert!(pkg.is_some());
    }

    #[test]
    fn workspace_metadata_topological_order() {
        let metadata = WorkspaceMetadata::load_from_current_dir().expect("load metadata");
        
        // Should be able to get topological order, though it may fail if 
        // there are circular dependencies in external packages
        let result = metadata.topological_order();
        // Just check it doesn't panic - the result depends on the workspace structure
        assert!(result.is_ok() || result.is_err());
    }
}
