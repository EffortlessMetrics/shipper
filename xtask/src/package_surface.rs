//! `cargo xtask package-surface`
//!
//! Reports the workspace package surface from `cargo metadata`. This is an
//! advisory inventory, not an API-diff gate: it gives release and policy work a
//! stable artifact naming the packages, publishability, targets, dependencies,
//! and feature surface currently present in the workspace.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const OUTPUT_DIR_REL: &str = "target/policy";
const REPORT_BASENAME: &str = "package-surface-report";

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<MetadataPackage>,
    workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MetadataPackage {
    id: String,
    name: String,
    version: String,
    manifest_path: String,
    publish: Option<Vec<String>>,
    #[serde(default)]
    targets: Vec<MetadataTarget>,
    #[serde(default)]
    dependencies: Vec<MetadataDependency>,
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct MetadataTarget {
    name: String,
    #[serde(default)]
    kind: Vec<String>,
    #[serde(default)]
    crate_types: Vec<String>,
    src_path: String,
}

#[derive(Debug, Deserialize)]
struct MetadataDependency {
    name: String,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Serialize)]
struct Report {
    tool: &'static str,
    generated_at: String,
    summary: Summary,
    packages: Vec<PackageSurface>,
}

#[derive(Debug, Serialize)]
struct Summary {
    workspace_packages: usize,
    publishable_packages: usize,
    private_packages: usize,
    public_targets: usize,
    dependency_edges: usize,
    feature_names: usize,
}

#[derive(Debug, Serialize)]
struct PackageSurface {
    name: String,
    version: String,
    manifest_path: String,
    publish: PublishSurface,
    targets: Vec<TargetSurface>,
    workspace_dependency_count: usize,
    external_dependency_count: usize,
    feature_count: usize,
    feature_names: Vec<String>,
    surface_hash: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "state")]
enum PublishSurface {
    Publishable { registries: Vec<String> },
    Private,
}

#[derive(Debug, Clone, Serialize)]
struct TargetSurface {
    name: String,
    kind: Vec<String>,
    crate_types: Vec<String>,
    src_path: String,
}

#[derive(Debug, Serialize)]
struct HashInput<'a> {
    name: &'a str,
    version: &'a str,
    manifest_path: &'a str,
    publish: &'a PublishSurface,
    targets: &'a [TargetSurface],
    workspace_dependency_count: usize,
    external_dependency_count: usize,
    feature_names: &'a [String],
}

pub fn package_surface() -> Result<()> {
    let workspace_root = workspace_root()?;
    let metadata = load_metadata(&workspace_root)?;
    let report = build_report(&workspace_root, metadata)?;
    write_report(&workspace_root, &report)?;

    println!(
        "wrote {}/{}.* ({} workspace packages, {} publishable)",
        OUTPUT_DIR_REL,
        REPORT_BASENAME,
        report.summary.workspace_packages,
        report.summary.publishable_packages,
    );
    Ok(())
}

fn build_report(workspace_root: &Path, metadata: Metadata) -> Result<Report> {
    let workspace_members = metadata
        .workspace_members
        .into_iter()
        .collect::<BTreeSet<_>>();

    let workspace_names = metadata
        .packages
        .iter()
        .filter(|pkg| workspace_members.contains(&pkg.id))
        .map(|pkg| pkg.name.clone())
        .collect::<BTreeSet<_>>();

    let mut packages = metadata
        .packages
        .into_iter()
        .filter(|pkg| workspace_members.contains(&pkg.id))
        .map(|pkg| package_surface_for(workspace_root, pkg, &workspace_names))
        .collect::<Result<Vec<_>>>()?;
    packages.sort_by(|a, b| a.name.cmp(&b.name));

    let summary = Summary {
        workspace_packages: packages.len(),
        publishable_packages: packages
            .iter()
            .filter(|pkg| matches!(pkg.publish, PublishSurface::Publishable { .. }))
            .count(),
        private_packages: packages
            .iter()
            .filter(|pkg| matches!(pkg.publish, PublishSurface::Private))
            .count(),
        public_targets: packages.iter().map(|pkg| pkg.targets.len()).sum(),
        dependency_edges: packages
            .iter()
            .map(|pkg| pkg.workspace_dependency_count + pkg.external_dependency_count)
            .sum(),
        feature_names: packages.iter().map(|pkg| pkg.feature_count).sum(),
    };

    Ok(Report {
        tool: "cargo xtask package-surface",
        generated_at: today_iso(),
        summary,
        packages,
    })
}

fn package_surface_for(
    workspace_root: &Path,
    pkg: MetadataPackage,
    workspace_names: &BTreeSet<String>,
) -> Result<PackageSurface> {
    let publish = publish_surface(&pkg.publish);
    let targets = pkg
        .targets
        .into_iter()
        .filter(is_public_target)
        .map(|target| {
            Ok(TargetSurface {
                name: target.name,
                kind: sorted(target.kind),
                crate_types: sorted(target.crate_types),
                src_path: workspace_relative_path(workspace_root, &target.src_path)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let mut feature_names = pkg.features.keys().cloned().collect::<Vec<_>>();
    feature_names.sort();

    let workspace_dependency_count = pkg
        .dependencies
        .iter()
        .filter(|dep| dep.path.is_some() || workspace_names.contains(&dep.name))
        .count();
    let external_dependency_count = pkg
        .dependencies
        .iter()
        .filter(|dep| dep.path.is_none() && !workspace_names.contains(&dep.name))
        .count();

    let mut surface = PackageSurface {
        name: pkg.name,
        version: pkg.version,
        manifest_path: workspace_relative_path(workspace_root, &pkg.manifest_path)?,
        publish,
        targets,
        workspace_dependency_count,
        external_dependency_count,
        feature_count: feature_names.len(),
        feature_names,
        surface_hash: String::new(),
    };
    surface.surface_hash = surface_hash(&surface)?;
    Ok(surface)
}

fn publish_surface(publish: &Option<Vec<String>>) -> PublishSurface {
    match publish {
        Some(registries) if registries.is_empty() => PublishSurface::Private,
        Some(registries) => PublishSurface::Publishable {
            registries: sorted(registries.clone()),
        },
        None => PublishSurface::Publishable {
            registries: Vec::new(),
        },
    }
}

fn is_public_target(target: &MetadataTarget) -> bool {
    target
        .kind
        .iter()
        .any(|kind| matches!(kind.as_str(), "bin" | "lib" | "proc-macro"))
}

fn sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}

fn surface_hash(surface: &PackageSurface) -> Result<String> {
    let input = HashInput {
        name: &surface.name,
        version: &surface.version,
        manifest_path: &surface.manifest_path,
        publish: &surface.publish,
        targets: &surface.targets,
        workspace_dependency_count: surface.workspace_dependency_count,
        external_dependency_count: surface.external_dependency_count,
        feature_names: &surface.feature_names,
    };
    let json = serde_json::to_vec(&input).context("serializing package surface hash input")?;
    Ok(fnv1a64_hex(&json))
}

fn fnv1a64_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn load_metadata(workspace_root: &Path) -> Result<Metadata> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version=1", "--no-deps"])
        .current_dir(workspace_root)
        .output()
        .context("running cargo metadata")?;
    if !output.status.success() {
        bail!(
            "`cargo metadata --format-version=1 --no-deps` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("parsing cargo metadata output")
}

fn write_report(workspace_root: &Path, report: &Report) -> Result<()> {
    let out_dir = workspace_root.join(OUTPUT_DIR_REL);
    fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    let json =
        serde_json::to_string_pretty(report).context("serializing package surface report")?;
    fs::write(out_dir.join(format!("{REPORT_BASENAME}.json")), json)
        .context("writing package-surface-report.json")?;
    fs::write(
        out_dir.join(format!("{REPORT_BASENAME}.md")),
        render_md(report),
    )
    .context("writing package-surface-report.md")?;
    Ok(())
}

fn render_md(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("# Package Surface Report\n\n");
    out.push_str(&format!(
        "Generated by `{}` on {}.\n\n",
        report.tool, report.generated_at
    ));
    out.push_str("## Summary\n\n");
    out.push_str(&format!(
        "- Workspace packages: {}\n",
        report.summary.workspace_packages
    ));
    out.push_str(&format!(
        "- Publishable packages: {}\n",
        report.summary.publishable_packages
    ));
    out.push_str(&format!(
        "- Private packages: {}\n",
        report.summary.private_packages
    ));
    out.push_str(&format!(
        "- Public targets: {}\n",
        report.summary.public_targets
    ));
    out.push_str(&format!(
        "- Dependency edges: {}\n",
        report.summary.dependency_edges
    ));
    out.push_str(&format!(
        "- Feature names: {}\n\n",
        report.summary.feature_names
    ));

    out.push_str("## Packages\n\n");
    out.push_str("| Package | Version | Publish | Targets | Workspace deps | External deps | Features | Surface hash |\n");
    out.push_str("|---|---|---|---:|---:|---:|---:|---|\n");
    for package in &report.packages {
        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} | {} | {} | `{}` |\n",
            package.name,
            package.version,
            render_publish(&package.publish),
            package.targets.len(),
            package.workspace_dependency_count,
            package.external_dependency_count,
            package.feature_count,
            package.surface_hash
        ));
    }
    out
}

fn render_publish(publish: &PublishSurface) -> String {
    match publish {
        PublishSurface::Private => "private".to_string(),
        PublishSurface::Publishable { registries } if registries.is_empty() => {
            "publishable".to_string()
        }
        PublishSurface::Publishable { registries } => {
            format!("publishable ({})", registries.join(", "))
        }
    }
}

fn workspace_relative_path(workspace_root: &Path, raw: &str) -> Result<String> {
    let path = PathBuf::from(raw);
    let rel = path.strip_prefix(workspace_root).unwrap_or(&path);
    Ok(rel.to_string_lossy().replace('\\', "/"))
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

fn today_iso() -> String {
    chrono::Utc::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{PublishSurface, fnv1a64_hex, publish_surface, sorted};

    #[test]
    fn publish_surface_treats_missing_publish_as_publishable() {
        assert_eq!(
            publish_surface(&None),
            PublishSurface::Publishable {
                registries: Vec::new()
            }
        );
    }

    #[test]
    fn publish_surface_treats_empty_publish_list_as_private() {
        assert_eq!(publish_surface(&Some(Vec::new())), PublishSurface::Private);
    }

    #[test]
    fn publish_surface_sorts_named_registries() {
        assert_eq!(
            publish_surface(&Some(vec!["z".to_string(), "a".to_string()])),
            PublishSurface::Publishable {
                registries: vec!["a".to_string(), "z".to_string()]
            }
        );
    }

    #[test]
    fn sorted_orders_values() {
        assert_eq!(
            sorted(vec!["b".to_string(), "a".to_string()]),
            vec!["a", "b"]
        );
    }

    #[test]
    fn fnv1a_hash_is_stable_for_known_input() {
        assert_eq!(fnv1a64_hex(b"shipper"), "51a3b918b49d87ee");
    }
}
