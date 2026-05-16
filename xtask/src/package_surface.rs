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
    kind: Option<String>,
}

#[derive(Debug, Serialize)]
struct Report {
    tool: &'static str,
    generated_at: String,
    summary: Summary,
    architecture_contract: ArchitectureContract,
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
struct ArchitectureContract {
    status: ContractStatus,
    checked_rules: Vec<&'static str>,
    violations: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ContractStatus {
    Pass,
    Fail,
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
    let violations = report.architecture_contract.violations.len();
    write_report(&workspace_root, &report)?;

    println!(
        "wrote {}/{}.* ({} workspace packages, {} publishable)",
        OUTPUT_DIR_REL,
        REPORT_BASENAME,
        report.summary.workspace_packages,
        report.summary.publishable_packages,
    );
    if violations > 0 {
        bail!("package-surface architecture contract failed with {violations} violation(s)");
    }
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
    let architecture_contract = check_architecture_contract(&metadata.packages, &workspace_members);

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
        architecture_contract,
        packages,
    })
}

fn check_architecture_contract(
    packages: &[MetadataPackage],
    workspace_members: &BTreeSet<String>,
) -> ArchitectureContract {
    let checked_rules = vec![
        "`shipper` exists and depends on `shipper-cli` plus `shipper-core`",
        "`shipper-cli` exists and depends on `shipper-core`",
        "`shipper-core` exists and has no normal, dev, or build dependency on `shipper`, `shipper-cli`, `clap`, or `indicatif`",
        "`xtask` is the only private workspace package",
    ];
    let mut violations = Vec::new();

    let shipper = workspace_package(packages, workspace_members, "shipper");
    let shipper_cli = workspace_package(packages, workspace_members, "shipper-cli");
    let shipper_core = workspace_package(packages, workspace_members, "shipper-core");

    require_package(shipper, "shipper", &mut violations);
    require_package(shipper_cli, "shipper-cli", &mut violations);
    require_package(shipper_core, "shipper-core", &mut violations);

    if let Some(package) = shipper {
        require_normal_dependency(package, "shipper-cli", &mut violations);
        require_normal_dependency(package, "shipper-core", &mut violations);
    }
    if let Some(package) = shipper_cli {
        require_normal_dependency(package, "shipper-core", &mut violations);
    }
    if let Some(package) = shipper_core {
        forbid_dependency(package, "shipper", &mut violations);
        forbid_dependency(package, "shipper-cli", &mut violations);
        forbid_dependency(package, "clap", &mut violations);
        forbid_dependency(package, "indicatif", &mut violations);
    }

    let private_packages = packages
        .iter()
        .filter(|pkg| workspace_members.contains(&pkg.id))
        .filter(|pkg| publish_surface(&pkg.publish) == PublishSurface::Private)
        .map(|pkg| pkg.name.as_str())
        .collect::<Vec<_>>();
    if private_packages != ["xtask"] {
        violations.push(format!(
            "private workspace packages must be exactly `xtask`; found: {}",
            if private_packages.is_empty() {
                "<none>".to_string()
            } else {
                private_packages.join(", ")
            }
        ));
    }

    ArchitectureContract {
        status: if violations.is_empty() {
            ContractStatus::Pass
        } else {
            ContractStatus::Fail
        },
        checked_rules,
        violations,
    }
}

fn workspace_package<'a>(
    packages: &'a [MetadataPackage],
    workspace_members: &BTreeSet<String>,
    name: &str,
) -> Option<&'a MetadataPackage> {
    packages
        .iter()
        .find(|pkg| workspace_members.contains(&pkg.id) && pkg.name == name)
}

fn require_package(package: Option<&MetadataPackage>, name: &str, violations: &mut Vec<String>) {
    if package.is_none() {
        violations.push(format!("required workspace package `{name}` is missing"));
    }
}

fn require_normal_dependency(
    package: &MetadataPackage,
    dependency: &str,
    violations: &mut Vec<String>,
) {
    if !package
        .dependencies
        .iter()
        .any(|dep| dep.name == dependency && dep.kind.as_deref().unwrap_or("normal") == "normal")
    {
        violations.push(format!(
            "`{}` must have a normal dependency on `{dependency}`",
            package.name
        ));
    }
}

fn forbid_dependency(package: &MetadataPackage, dependency: &str, violations: &mut Vec<String>) {
    if package
        .dependencies
        .iter()
        .any(|dep| dep.name == dependency)
    {
        violations.push(format!(
            "`{}` must not depend on `{dependency}`",
            package.name
        ));
    }
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

    out.push_str("## Architecture Contract\n\n");
    out.push_str(&format!(
        "- Status: `{}`\n",
        render_contract_status(report.architecture_contract.status)
    ));
    out.push_str("- Checked rules:\n");
    for rule in &report.architecture_contract.checked_rules {
        out.push_str(&format!("  - {rule}\n"));
    }
    if report.architecture_contract.violations.is_empty() {
        out.push_str("- Violations: none\n\n");
    } else {
        out.push_str("- Violations:\n");
        for violation in &report.architecture_contract.violations {
            out.push_str(&format!("  - {violation}\n"));
        }
        out.push('\n');
    }

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

fn render_contract_status(status: ContractStatus) -> &'static str {
    match status {
        ContractStatus::Pass => "pass",
        ContractStatus::Fail => "fail",
    }
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
    use std::collections::BTreeSet;

    use super::{
        ContractStatus, MetadataDependency, MetadataPackage, PublishSurface,
        check_architecture_contract, fnv1a64_hex, publish_surface, sorted,
    };

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

    #[test]
    fn architecture_contract_passes_for_facade_cli_core_shape() {
        let packages = product_graph_packages(vec![]);
        let workspace_members = workspace_members(&packages);

        let contract = check_architecture_contract(&packages, &workspace_members);

        assert_eq!(contract.status, ContractStatus::Pass);
        assert!(contract.violations.is_empty());
    }

    #[test]
    fn architecture_contract_rejects_core_cli_dependencies() {
        let packages = product_graph_packages(vec![normal_dep("shipper-cli"), normal_dep("clap")]);
        let workspace_members = workspace_members(&packages);

        let contract = check_architecture_contract(&packages, &workspace_members);

        assert_eq!(contract.status, ContractStatus::Fail);
        assert!(contract.violations.iter().any(|violation| {
            violation.contains("`shipper-core` must not depend on `shipper-cli`")
        }));
        assert!(
            contract
                .violations
                .iter()
                .any(|violation| violation.contains("`shipper-core` must not depend on `clap`"))
        );
    }

    #[test]
    fn architecture_contract_requires_xtask_as_only_private_package() {
        let mut packages = product_graph_packages(vec![]);
        packages.push(package("helper-task", Some(Vec::new()), vec![]));
        let workspace_members = workspace_members(&packages);

        let contract = check_architecture_contract(&packages, &workspace_members);

        assert_eq!(contract.status, ContractStatus::Fail);
        assert!(contract.violations.iter().any(|violation| {
            violation.contains("private workspace packages must be exactly `xtask`")
        }));
    }

    fn product_graph_packages(core_dependencies: Vec<MetadataDependency>) -> Vec<MetadataPackage> {
        vec![
            package(
                "shipper",
                None,
                vec![normal_dep("shipper-cli"), normal_dep("shipper-core")],
            ),
            package("shipper-cli", None, vec![normal_dep("shipper-core")]),
            package("shipper-core", None, core_dependencies),
            package("xtask", Some(Vec::new()), vec![]),
        ]
    }

    fn package(
        name: &str,
        publish: Option<Vec<String>>,
        dependencies: Vec<MetadataDependency>,
    ) -> MetadataPackage {
        MetadataPackage {
            id: format!("path+file:///workspace/{name}#0.0.0"),
            name: name.to_string(),
            version: "0.0.0".to_string(),
            manifest_path: format!("/workspace/{name}/Cargo.toml"),
            publish,
            targets: Vec::new(),
            dependencies,
            features: Default::default(),
        }
    }

    fn normal_dep(name: &str) -> MetadataDependency {
        MetadataDependency {
            name: name.to_string(),
            path: Some(format!("/workspace/{name}")),
            kind: None,
        }
    }

    fn workspace_members(packages: &[MetadataPackage]) -> BTreeSet<String> {
        packages.iter().map(|package| package.id.clone()).collect()
    }
}
