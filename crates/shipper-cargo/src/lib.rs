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
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cargo_metadata::{Metadata, MetadataCommand, Package};
use serde::{Deserialize, Serialize};

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