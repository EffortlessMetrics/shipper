//! Plan-yank: reverse-topological containment plan from a receipt (#98 PR 2).
//!
//! Given a receipt.json from a prior publish run, produce an ordered list of
//! `<crate>@<version>` entries describing the yank order for containment:
//! dependents first, dependencies last. This is the opposite of publish
//! order — we want downstream consumers of the bad version to stop being
//! resolvable against it *before* we yank the bad version itself.
//!
//! ## Example
//!
//! For a workspace A → B → C (A is a leaf, B depends on A, C depends on B):
//!
//! - Publish order (receipt.packages): `[A, B, C]`
//! - Yank order (reverse topological): `[C, B, A]`
//!
//! ## What this PR does and does not do
//!
//! **Does:**
//! - Read a receipt
//! - Filter packages (all published, or only those with
//!   `compromised_at = Some(_)`)
//! - Return the entries in reverse-topological order
//! - Provide both a structured `YankPlan` API and a text renderer
//!
//! **Does not (yet):**
//! - Execute the plan — that's `shipper yank` (already landed) running
//!   one entry at a time. Plan execution wrapping is #98 PR 3.
//! - Mark a package compromised — that's `--mark-compromised`, landing
//!   in #98 PR 3 alongside fix-forward.
//!
//! Keeping this PR to **planning only** matches the staged rollout agreed
//! in the #98 scope: primitive → plan → execute / fix-forward.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use shipper_types::{PackageReceipt, PackageState, Receipt};

/// One entry in a reverse-topological yank plan.
///
/// Both `Serialize` and `Deserialize` because plan files are meant to
/// round-trip: planner writes JSON, operator reviews, executor reads
/// it back (#98 PR 5).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct YankEntry {
    pub name: String,
    pub version: String,
    /// If the receipt marked this package compromised, the reason string
    /// surfaces here so the operator running the plan sees per-crate
    /// context (CVE id, ticket, etc.) without having to cross-reference
    /// the receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Selection predicate for which receipt packages to include in the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanYankFilter {
    /// Every package whose terminal state is `Published` gets a yank
    /// entry. This is the "yank the whole release" case, e.g. a full
    /// rollback.
    AllPublished,
    /// Only packages with a `compromised_at = Some(_)` field get an
    /// entry. Used when a specific subset of a release is compromised
    /// (a CVE in one crate, say) and the rest is fine.
    CompromisedOnly,
}

/// A reverse-topological yank plan derived from a receipt.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct YankPlan {
    pub plan_id: String,
    pub registry: String,
    /// Which selector produced this plan. Serialized as a `String` in
    /// both directions so the plan file round-trips cleanly even across
    /// Shipper versions that add new selector modes.
    #[serde(default = "unknown_filter")]
    pub filter: std::borrow::Cow<'static, str>,
    pub entries: Vec<YankEntry>,
}

fn unknown_filter() -> std::borrow::Cow<'static, str> {
    std::borrow::Cow::Borrowed("unknown")
}

fn include(receipt: &PackageReceipt, filter: PlanYankFilter) -> bool {
    match filter {
        PlanYankFilter::AllPublished => matches!(receipt.state, PackageState::Published),
        PlanYankFilter::CompromisedOnly => receipt.compromised_at.is_some(),
    }
}

/// Build a reverse-topological yank plan from a receipt.
///
/// The receipt's `packages` vector is in publish (topological) order, so
/// we filter then reverse. Failing and skipped packages are excluded by
/// default — yanking a version that was never published is a no-op on
/// the registry and would just produce noise.
pub fn build_plan(receipt: &Receipt, filter: PlanYankFilter) -> YankPlan {
    let mut entries: Vec<YankEntry> = receipt
        .packages
        .iter()
        .filter(|p| include(p, filter))
        .map(|p| YankEntry {
            name: p.name.clone(),
            version: p.version.clone(),
            reason: p.compromised_by.clone(),
        })
        .collect();
    entries.reverse();

    YankPlan {
        plan_id: receipt.plan_id.clone(),
        registry: receipt.registry.name.clone(),
        filter: std::borrow::Cow::Borrowed(match filter {
            PlanYankFilter::AllPublished => "all_published",
            PlanYankFilter::CompromisedOnly => "compromised_only",
        }),
        entries,
    }
}

/// Build a reverse-topological yank plan rooted at a specific broken crate
/// (#98 PR 4). Walks the release plan's dependency graph backwards from
/// `starting_crate` to enumerate every crate that transitively depends on
/// it, then orders them dependents-first.
///
/// This is the **graph mode** complement to `build_plan`'s receipt-filter
/// modes. Use when you know exactly which crate is broken (e.g. CVE
/// targeting `my-lib`) and want containment of only the affected
/// dependency chain — not a full-release rollback.
///
/// `dependency_graph` is `plan.dependencies` from the original
/// `ReleasePlan` (crate → list of its intra-workspace deps). Not
/// embedded in `Receipt` because receipts are summaries, not graphs.
///
/// Errors if `starting_crate` is not in the receipt.
pub fn build_plan_from_starting_crate(
    receipt: &Receipt,
    dependency_graph: &BTreeMap<String, Vec<String>>,
    starting_crate: &str,
    reason: Option<String>,
) -> Result<YankPlan> {
    if !receipt.packages.iter().any(|p| p.name == starting_crate) {
        bail!(
            "starting crate '{starting_crate}' is not in this receipt; \
             available packages: {}",
            receipt
                .packages
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // Reverse-walk the dependency graph: starting from the broken crate,
    // collect every crate that (transitively) depends on it.
    let mut affected: BTreeSet<String> = BTreeSet::new();
    affected.insert(starting_crate.to_string());
    let mut frontier: Vec<String> = vec![starting_crate.to_string()];
    while let Some(current) = frontier.pop() {
        for (dependent, deps) in dependency_graph.iter() {
            if deps.iter().any(|d| d == &current) && affected.insert(dependent.clone()) {
                frontier.push(dependent.clone());
            }
        }
    }

    // receipt.packages is in topological order. Filter to the affected set
    // and restrict to actually-Published entries (yanking a Failed / never-
    // shipped entry is a no-op on the registry). Then reverse so
    // dependents come first.
    let mut entries: Vec<YankEntry> = receipt
        .packages
        .iter()
        .filter(|p| affected.contains(&p.name))
        .filter(|p| matches!(p.state, PackageState::Published))
        .map(|p| YankEntry {
            name: p.name.clone(),
            version: p.version.clone(),
            // Per-entry reason priority:
            //   1. Operator-supplied `--reason <text>` (applies to all)
            //   2. Existing `compromised_by` on the receipt entry
            //   3. None
            reason: reason.clone().or_else(|| p.compromised_by.clone()),
        })
        .collect();
    entries.reverse();

    Ok(YankPlan {
        plan_id: receipt.plan_id.clone(),
        registry: receipt.registry.name.clone(),
        filter: std::borrow::Cow::Borrowed("starting_crate"),
        entries,
    })
}

/// Load a saved yank plan from disk (#98 PR 5).
///
/// Used by `shipper yank --plan <path>` to drive execution over a
/// reviewed plan file produced by `shipper plan-yank`. The file format
/// is the same JSON shape `plan-yank --format json` produces, so plans
/// round-trip without any munging.
pub fn load_plan_from_path(path: &std::path::Path) -> Result<YankPlan> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read yank plan at {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse yank plan at {}", path.display()))
}

/// Load a receipt from an arbitrary path (not necessarily inside a state dir).
/// `shipper plan-yank --from-receipt path/to/receipt.json` uses this.
pub fn load_receipt_from_path(path: &std::path::Path) -> Result<Receipt> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read receipt at {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse receipt at {}", path.display()))
}

/// Render a yank plan as a human-readable text block. The first column is
/// the yank order (1-indexed); the intent is that an operator can eyeball
/// the plan and cross-reference with their change-management process.
pub fn render_text(plan: &YankPlan) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# yank plan (reverse topological) — registry={}, plan_id={}, filter={}\n",
        plan.registry, plan.plan_id, plan.filter
    ));
    out.push_str(&format!("# {} entries\n", plan.entries.len()));
    if plan.entries.is_empty() {
        out.push_str("# (no packages match the filter; nothing to yank)\n");
        return out;
    }
    for (i, e) in plan.entries.iter().enumerate() {
        let reason = e
            .reason
            .as_deref()
            .map(|r| format!("  # {r}"))
            .unwrap_or_default();
        out.push_str(&format!(
            "{:>3}. shipper yank --crate {} --version {} --reason <REASON>{reason}\n",
            i + 1,
            e.name,
            e.version
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use shipper_types::{
        EnvironmentFingerprint, PackageEvidence, PackageReceipt, PackageState, Receipt, Registry,
    };
    use std::path::PathBuf;

    fn pkg(name: &str, state: PackageState, compromised: Option<&str>) -> PackageReceipt {
        PackageReceipt {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            attempts: 1,
            state,
            started_at: Utc::now(),
            finished_at: Utc::now(),
            duration_ms: 10,
            evidence: PackageEvidence {
                attempts: vec![],
                readiness_checks: vec![],
            },
            compromised_at: compromised.map(|_| Utc::now()),
            compromised_by: compromised.map(str::to_string),
            superseded_by: None,
        }
    }

    fn sample_receipt(packages: Vec<PackageReceipt>) -> Receipt {
        Receipt {
            receipt_version: "shipper.receipt.v2".to_string(),
            plan_id: "plan-sample".to_string(),
            registry: Registry::crates_io(),
            started_at: Utc::now(),
            finished_at: Utc::now(),
            packages,
            event_log_path: PathBuf::from(".shipper/events.jsonl"),
            git_context: None,
            environment: EnvironmentFingerprint {
                shipper_version: "0.3.0".into(),
                cargo_version: None,
                rust_version: None,
                os: "test".into(),
                arch: "x86_64".into(),
            },
        }
    }

    #[test]
    fn reverses_publish_order_for_all_published() {
        let r = sample_receipt(vec![
            pkg("a", PackageState::Published, None),
            pkg("b", PackageState::Published, None),
            pkg("c", PackageState::Published, None),
        ]);
        let plan = build_plan(&r, PlanYankFilter::AllPublished);
        let names: Vec<_> = plan.entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["c", "b", "a"]);
    }

    #[test]
    fn excludes_failed_and_skipped_packages() {
        let r = sample_receipt(vec![
            pkg("a", PackageState::Published, None),
            pkg(
                "b",
                PackageState::Failed {
                    class: shipper_types::ErrorClass::Permanent,
                    message: "nope".into(),
                },
                None,
            ),
            pkg(
                "c",
                PackageState::Skipped {
                    reason: "already there".into(),
                },
                None,
            ),
        ]);
        let plan = build_plan(&r, PlanYankFilter::AllPublished);
        let names: Vec<_> = plan.entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["a"]);
    }

    #[test]
    fn compromised_only_filter_drops_healthy_packages() {
        let r = sample_receipt(vec![
            pkg("a", PackageState::Published, None),
            pkg("b", PackageState::Published, Some("CVE-2026-0001")),
            pkg("c", PackageState::Published, None),
        ]);
        let plan = build_plan(&r, PlanYankFilter::CompromisedOnly);
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].name, "b");
        assert_eq!(plan.entries[0].reason.as_deref(), Some("CVE-2026-0001"));
    }

    #[test]
    fn empty_plan_on_empty_receipt() {
        let r = sample_receipt(vec![]);
        let plan = build_plan(&r, PlanYankFilter::AllPublished);
        assert!(plan.entries.is_empty());
        assert!(render_text(&plan).contains("nothing to yank"));
    }

    #[test]
    fn text_render_uses_reverse_topo_order_with_indices() {
        let r = sample_receipt(vec![
            pkg("a", PackageState::Published, None),
            pkg("b", PackageState::Published, None),
        ]);
        let out = render_text(&build_plan(&r, PlanYankFilter::AllPublished));
        // dependents (b) before dependencies (a), 1-indexed
        let b_pos = out.find("shipper yank --crate b").unwrap();
        let a_pos = out.find("shipper yank --crate a").unwrap();
        assert!(
            b_pos < a_pos,
            "b must come before a in reverse topo:\n{out}"
        );
        assert!(out.starts_with("# yank plan"));
    }

    // ── build_plan_from_starting_crate tests (#98 PR 4) ─────────────────

    /// Graph: a (leaf) ← b ← c. Dep map says: b depends on a, c depends
    /// on b and a. Starting from "a" (the leaf), all three should be in
    /// the plan, with c yanked first, then b, then a.
    #[test]
    fn starting_crate_walks_all_transitive_dependents_in_reverse_topo() {
        let r = sample_receipt(vec![
            pkg("a", PackageState::Published, None),
            pkg("b", PackageState::Published, None),
            pkg("c", PackageState::Published, None),
        ]);
        let mut deps = BTreeMap::new();
        deps.insert("a".to_string(), vec![]);
        deps.insert("b".to_string(), vec!["a".to_string()]);
        deps.insert("c".to_string(), vec!["a".to_string(), "b".to_string()]);

        let plan = build_plan_from_starting_crate(&r, &deps, "a", None).expect("plan");
        let names: Vec<_> = plan.entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["c", "b", "a"]);
        assert_eq!(plan.filter, "starting_crate");
    }

    /// Graph: a ← b (b depends on a), plus an unrelated crate z. Starting
    /// from "a" should yank a and b but NOT z (no dependency edge).
    #[test]
    fn starting_crate_ignores_unrelated_crates() {
        let r = sample_receipt(vec![
            pkg("a", PackageState::Published, None),
            pkg("b", PackageState::Published, None),
            pkg("z", PackageState::Published, None),
        ]);
        let mut deps = BTreeMap::new();
        deps.insert("a".to_string(), vec![]);
        deps.insert("b".to_string(), vec!["a".to_string()]);
        deps.insert("z".to_string(), vec![]); // independent

        let plan = build_plan_from_starting_crate(&r, &deps, "a", None).expect("plan");
        let names: Vec<_> = plan.entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["b", "a"]);
        assert!(!names.contains(&"z".to_string()));
    }

    #[test]
    fn starting_crate_skips_non_published_entries() {
        let r = sample_receipt(vec![
            pkg("a", PackageState::Published, None),
            pkg(
                "b",
                PackageState::Failed {
                    class: shipper_types::ErrorClass::Permanent,
                    message: "nope".into(),
                },
                None,
            ),
        ]);
        let mut deps = BTreeMap::new();
        deps.insert("b".to_string(), vec!["a".to_string()]);

        let plan = build_plan_from_starting_crate(&r, &deps, "a", None).expect("plan");
        // b is a dependent of a but it Failed to publish — never on
        // registry — so it's excluded from the yank plan. Only a remains.
        let names: Vec<_> = plan.entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["a"]);
    }

    #[test]
    fn starting_crate_applies_explicit_reason_to_every_entry() {
        let r = sample_receipt(vec![
            pkg("a", PackageState::Published, None),
            pkg("b", PackageState::Published, None),
        ]);
        let mut deps = BTreeMap::new();
        deps.insert("b".to_string(), vec!["a".to_string()]);

        let plan =
            build_plan_from_starting_crate(&r, &deps, "a", Some("CVE-2026-0001".to_string()))
                .expect("plan");
        assert_eq!(plan.entries.len(), 2);
        for entry in &plan.entries {
            assert_eq!(entry.reason.as_deref(), Some("CVE-2026-0001"));
        }
    }

    #[test]
    fn starting_crate_errors_when_not_in_receipt() {
        let r = sample_receipt(vec![pkg("a", PackageState::Published, None)]);
        let deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let err =
            build_plan_from_starting_crate(&r, &deps, "bogus", None).expect_err("should error");
        let msg = format!("{err:#}");
        assert!(msg.contains("not in this receipt"), "err: {msg}");
    }

    // ── load_plan_from_path (#98 PR 5) roundtrip ──────────────────────────

    #[test]
    fn yank_plan_json_roundtrips_via_load_plan_from_path() {
        let td = tempfile::tempdir().expect("tempdir");
        let r = sample_receipt(vec![
            pkg("a", PackageState::Published, Some("CVE-1")),
            pkg("b", PackageState::Published, None),
        ]);
        let plan = build_plan(&r, PlanYankFilter::AllPublished);
        let path = td.path().join("yank-plan.json");
        let raw = serde_json::to_string_pretty(&plan).expect("serialize");
        std::fs::write(&path, raw).expect("write");

        let loaded = load_plan_from_path(&path).expect("load");
        assert_eq!(loaded.plan_id, plan.plan_id);
        assert_eq!(loaded.registry, plan.registry);
        assert_eq!(loaded.entries.len(), plan.entries.len());
        // Entry order preserved (dependents first)
        assert_eq!(loaded.entries[0].name, "b");
        assert_eq!(loaded.entries[1].name, "a");
        // Per-entry reason preserved
        assert_eq!(loaded.entries[1].reason.as_deref(), Some("CVE-1"));
    }

    #[test]
    fn load_plan_from_path_errors_on_missing_file() {
        let err = load_plan_from_path(std::path::Path::new("/definitely/not/there.json"))
            .expect_err("should fail");
        assert!(format!("{err:#}").contains("failed to read yank plan"));
    }

    #[test]
    fn load_plan_from_path_errors_on_malformed_json() {
        let td = tempfile::tempdir().expect("tempdir");
        let path = td.path().join("malformed.json");
        std::fs::write(&path, "{ not valid json ").expect("write");
        let err = load_plan_from_path(&path).expect_err("should fail");
        assert!(format!("{err:#}").contains("failed to parse yank plan"));
    }
}
