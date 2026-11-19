//! Shared model for the exact, owned workflow-authority exception ledger.
//!
//! One parser, two consumers. The `workflow-authority-exceptions` binary is the
//! schema/expiry ratchet; `check-workflow-surfaces` reconciles these exact
//! identities against the detector's emitted findings. A second, independently
//! drifting parser for a security control would itself be a defect, so this
//! module is the only place the ledger is read or validated.
//!
//! Each consumer uses a subset of the surface, hence the module-wide
//! `dead_code` allowance.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde::Deserialize;

pub(crate) const LEDGER: &str = "policy/workflow-authority-exceptions.toml";
pub(crate) const EXPECTED_SCHEMA: &str = "1.0";
pub(crate) const EXPECTED_POLICY: &str = "workflow-authority-exceptions";
const SECONDS_PER_DAY: u64 = 86_400;

/// Read and parse the ledger without validating it.
///
/// Parsing and validation stay separate so the blocking reconciler can report
/// an invalid ledger as a finding instead of panicking, and still render the
/// rest of its report.
pub(crate) fn load(workspace_root: &Path) -> Result<AuthorityExceptionDoc> {
    let path = workspace_root.join(LEDGER);
    let text = fs::read_to_string(&path)
        .with_context(|| format!("reading workflow authority ledger {}", path.display()))?;
    toml::from_str(&text)
        .with_context(|| format!("parsing workflow authority ledger {}", path.display()))
}

#[derive(Debug, Deserialize)]
pub(crate) struct AuthorityExceptionDoc {
    pub(crate) schema_version: String,
    pub(crate) policy: String,
    pub(crate) owner: String,
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) authority_exception: Vec<AuthorityException>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AuthorityException {
    pub(crate) workflow: String,
    pub(crate) job: String,
    pub(crate) step: String,
    pub(crate) capability: String,
    pub(crate) trigger: String,
    pub(crate) repository: String,
    pub(crate) finding_repository_boundary: String,
    pub(crate) owner: String,
    pub(crate) reason: String,
    pub(crate) covered_by: String,
    pub(crate) created: String,
    pub(crate) review_after: String,
}

/// The exact six-field authority identity, shared by the ledger and the detector.
///
/// Both sides must format this identically: reconciliation compares the two
/// strings for equality, so a divergence would not fail to compile — it would
/// silently stop every record matching its finding and report the whole ledger
/// as unused. One function keeps that impossible.
pub(crate) fn identity_of(
    workflow: &str,
    job: &str,
    step: &str,
    capability: &str,
    trigger: &str,
    finding_repository_boundary: &str,
) -> String {
    format!("{workflow}|{job}|{step}|{capability}|{trigger}|{finding_repository_boundary}")
}

impl AuthorityException {
    pub(crate) fn identity(&self) -> String {
        identity_of(
            &self.workflow,
            &self.job,
            &self.step,
            &self.capability,
            &self.trigger,
            &self.finding_repository_boundary,
        )
    }
}

pub(crate) fn current_utc_date() -> Result<NaiveDate> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?;
    let unix_days = i64::try_from(elapsed.as_secs() / SECONDS_PER_DAY)
        .context("current Unix day does not fit in i64")?;
    date_from_unix_days(unix_days).context("current UTC date is outside Chrono's supported range")
}

/// Convert whole UTC days since 1970-01-01 to a civil date.
///
/// This is Howard Hinnant's public-domain civil-from-days algorithm. Keeping
/// the conversion here avoids requiring Chrono's wall-clock feature merely to
/// expire a private repository policy record.
fn date_from_unix_days(unix_days: i64) -> Option<NaiveDate> {
    let days = unix_days.checked_add(719_468)?;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);

    NaiveDate::from_ymd_opt(
        i32::try_from(year).ok()?,
        u32::try_from(month).ok()?,
        u32::try_from(day).ok()?,
    )
}

/// Resolve the workspace root the same way every other `xtask` helper does.
///
/// Walking up from `current_dir()` would resolve to whichever checkout the
/// caller happens to stand in, which differs from the rest of `xtask` when the
/// workspace is nested or vendored. The ledger path must resolve identically to
/// the detector's.
pub(crate) fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .context("CARGO_MANIFEST_DIR not set; run via `cargo xtask`")?;
    let xtask_dir = PathBuf::from(manifest_dir);
    let root = xtask_dir
        .parent()
        .with_context(|| format!("xtask manifest dir has no parent: {}", xtask_dir.display()))?
        .to_path_buf();
    Ok(root)
}

pub(crate) fn validate_doc<F>(
    doc: &AuthorityExceptionDoc,
    today: NaiveDate,
    workflow_exists: F,
) -> Result<usize>
where
    F: Fn(&str) -> bool,
{
    if doc.schema_version != EXPECTED_SCHEMA {
        bail!(
            "unsupported authority-exception schema {}; expected {EXPECTED_SCHEMA}",
            doc.schema_version
        );
    }
    if doc.policy != EXPECTED_POLICY {
        bail!(
            "unexpected authority-exception policy {}; expected {EXPECTED_POLICY}",
            doc.policy
        );
    }
    require_text("ledger owner", &doc.owner, 3)?;
    if doc.status != "active" {
        bail!("authority-exception ledger status must be active");
    }

    // Validate every entry before returning. Bailing on the first fault made an
    // operator fix one record, rerun, and discover the next one — a ledger with
    // three bad records took three runs to diagnose.
    let mut identities = BTreeSet::new();
    let mut errors: Vec<String> = Vec::new();
    for entry in &doc.authority_exception {
        if let Err(error) = validate_exception(entry, today, &workflow_exists) {
            errors.push(format!("{}: {error:#}", entry.identity()));
        }
        let identity = entry.identity();
        if !identities.insert(identity.clone()) {
            errors.push(format!(
                "duplicate workflow authority exception: {identity}"
            ));
        }
    }
    if !errors.is_empty() {
        bail!("{}", errors.join("; "));
    }
    Ok(doc.authority_exception.len())
}

fn validate_exception<F>(
    entry: &AuthorityException,
    today: NaiveDate,
    workflow_exists: &F,
) -> Result<()>
where
    F: Fn(&str) -> bool,
{
    for (label, value, minimum) in [
        ("workflow", entry.workflow.as_str(), 1),
        ("job", entry.job.as_str(), 1),
        ("step", entry.step.as_str(), 1),
        ("capability", entry.capability.as_str(), 3),
        ("trigger", entry.trigger.as_str(), 1),
        ("repository", entry.repository.as_str(), 3),
        (
            "finding_repository_boundary",
            entry.finding_repository_boundary.as_str(),
            3,
        ),
        ("owner", entry.owner.as_str(), 3),
        ("reason", entry.reason.as_str(), 40),
        ("covered_by", entry.covered_by.as_str(), 40),
    ] {
        require_text(label, value, minimum)?;
    }

    if !entry.workflow.starts_with(".github/workflows/") || !entry.workflow.ends_with(".yml") {
        bail!(
            "authority exception workflow must name one exact .github/workflows/*.yml path: {}",
            entry.workflow
        );
    }
    if entry
        .workflow
        .chars()
        .any(|character| matches!(character, '*' | '?' | '[' | ']'))
    {
        bail!(
            "authority exception workflow may not contain a glob: {}",
            entry.workflow
        );
    }
    if !workflow_exists(&entry.workflow) {
        bail!(
            "authority exception references a missing workflow: {}",
            entry.workflow
        );
    }
    if entry.capability.contains('*') {
        bail!(
            "authority exception capability must be exact, not wildcarded: {}",
            entry.capability
        );
    }
    if !entry.repository.starts_with("EffortlessMetrics/") {
        bail!(
            "authority exception repository must name the exact EffortlessMetrics repository: {}",
            entry.repository
        );
    }

    let trigger_parts = entry.trigger.split(',').collect::<Vec<_>>();
    if trigger_parts.iter().any(|part| part.trim().is_empty()) {
        bail!("authority exception trigger contains an empty token");
    }
    // Reject padding rather than trimming it. `check-workflow-surfaces` compares
    // `trigger` byte for byte, so silently trimming here would accept a record
    // that can never match its finding. Rejecting also keeps the sorted-token
    // error below from firing with a misleading message: " workflow_dispatch"
    // sorts before "schedule", so `schedule, workflow_dispatch` would otherwise
    // be reported as unsorted when the real fault is the space.
    if trigger_parts.iter().any(|part| part.trim() != *part) {
        bail!(
            "authority exception trigger tokens may not be padded with whitespace: {}",
            entry.trigger
        );
    }
    let mut sorted_triggers = trigger_parts.clone();
    sorted_triggers.sort_unstable();
    sorted_triggers.dedup();
    if trigger_parts != sorted_triggers {
        bail!(
            "authority exception trigger must use sorted unique detector tokens: {}",
            entry.trigger
        );
    }

    let created = parse_date("created", &entry.created)?;
    let review_after = parse_date("review_after", &entry.review_after)?;
    if review_after <= created {
        bail!(
            "authority exception review_after {} must be after created {}",
            entry.review_after,
            entry.created
        );
    }
    if review_after < today {
        bail!(
            "authority exception expired on {}; today is {today}",
            entry.review_after
        );
    }

    for value in [
        &entry.reason,
        &entry.covered_by,
        &entry.finding_repository_boundary,
    ] {
        if contains_secret_material(value) {
            bail!("authority exception contains secret-like material");
        }
    }

    Ok(())
}

fn require_text(label: &str, value: &str, minimum: usize) -> Result<()> {
    if value.trim().len() < minimum {
        bail!("authority exception {label} must contain at least {minimum} characters");
    }
    Ok(())
}

fn parse_date(label: &str, value: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .with_context(|| format!("authority exception {label} must be YYYY-MM-DD: {value}"))
}

fn contains_secret_material(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "github_pat_",
        "ghp_",
        "authorization: bearer",
        "cookie:",
        "token=",
        "password=",
        "passphrase=",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: &str = "2026-08-06";

    fn valid_doc() -> AuthorityExceptionDoc {
        AuthorityExceptionDoc {
            schema_version: EXPECTED_SCHEMA.to_string(),
            policy: EXPECTED_POLICY.to_string(),
            owner: "release/ci".to_string(),
            status: "active".to_string(),
            authority_exception: vec![AuthorityException {
                workflow: ".github/workflows/droid-security-scan.yml".to_string(),
                job: "droid-security-scan".to_string(),
                step: "<permissions>".to_string(),
                capability: "contents:write".to_string(),
                trigger: "schedule,workflow_dispatch".to_string(),
                repository: "EffortlessMetrics/shipper-swarm".to_string(),
                finding_repository_boundary: "not declared".to_string(),
                owner: "release/ci".to_string(),
                reason:
                    "A durable security-report branch requires exact repository write authority."
                        .to_string(),
                covered_by:
                    "Scheduled/manual triggers, fixed action SHA, trusted runner, and review."
                        .to_string(),
                created: "2026-08-06".to_string(),
                review_after: "2026-11-06".to_string(),
            }],
        }
    }

    fn today() -> NaiveDate {
        NaiveDate::parse_from_str(TODAY, "%Y-%m-%d").expect("valid date")
    }

    #[test]
    fn unix_epoch_converts_to_the_expected_civil_date() {
        assert_eq!(date_from_unix_days(0), NaiveDate::from_ymd_opt(1970, 1, 1));
    }

    #[test]
    fn accepts_one_exact_current_exception() {
        assert_eq!(
            validate_doc(&valid_doc(), today(), |_| true).expect("valid"),
            1
        );
    }

    #[test]
    fn rejects_duplicate_exception_identity() {
        let mut doc = valid_doc();
        doc.authority_exception
            .push(doc.authority_exception[0].clone());
        let error = validate_doc(&doc, today(), |_| true).expect_err("duplicate must fail");
        assert!(error.to_string().contains("duplicate"), "{error:#}");
    }

    #[test]
    fn rejects_globbed_workflow_authority() {
        let mut doc = valid_doc();
        doc.authority_exception[0].workflow = ".github/workflows/*.yml".to_string();
        let error = validate_doc(&doc, today(), |_| true).expect_err("glob must fail");
        assert!(error.to_string().contains("glob"), "{error:#}");
    }

    #[test]
    fn rejects_expired_exception() {
        let mut doc = valid_doc();
        // `created` must move back too. The fixture is created on TODAY, so a
        // past `review_after` would trip the ordering rule first and never
        // reach the expiry check this test is about.
        doc.authority_exception[0].created = "2026-01-01".to_string();
        doc.authority_exception[0].review_after = "2026-08-05".to_string();
        let error = validate_doc(&doc, today(), |_| true).expect_err("expiry must fail");
        assert!(error.to_string().contains("expired"), "{error:#}");
    }

    #[test]
    fn rejects_review_after_on_or_before_created() {
        for review_after in ["2026-08-06", "2026-08-05"] {
            let mut doc = valid_doc();
            doc.authority_exception[0].created = "2026-08-06".to_string();
            doc.authority_exception[0].review_after = review_after.to_string();
            let error = validate_doc(&doc, today(), |_| true).expect_err("ordering must fail");
            assert!(
                error.to_string().contains("must be after created"),
                "{error:#}"
            );
        }
    }

    #[test]
    fn rejects_secret_like_exception_text() {
        let mut doc = valid_doc();
        doc.authority_exception[0].reason =
            "A long enough explanation that accidentally contains token=secret".to_string();
        let error = validate_doc(&doc, today(), |_| true).expect_err("secret must fail");
        assert!(error.to_string().contains("secret-like"), "{error:#}");
    }

    #[test]
    fn rejects_a_record_naming_a_missing_workflow() {
        let error =
            validate_doc(&valid_doc(), today(), |_| false).expect_err("missing workflow must fail");
        assert!(
            error.to_string().contains("does not exist")
                || error.to_string().contains("missing")
                || error.to_string().contains("not a tracked workflow"),
            "{error:#}"
        );
    }

    #[test]
    fn rejects_a_repository_outside_the_organization() {
        let mut doc = valid_doc();
        doc.authority_exception[0].repository = "Attacker/evil".to_string();
        let error = validate_doc(&doc, today(), |_| true).expect_err("foreign repo must fail");
        assert!(error.to_string().contains("EffortlessMetrics"), "{error:#}");
    }

    #[test]
    fn rejects_a_wildcarded_capability() {
        let mut doc = valid_doc();
        doc.authority_exception[0].capability = "contents:*".to_string();
        let error = validate_doc(&doc, today(), |_| true).expect_err("glob capability must fail");
        assert!(error.to_string().contains("wildcarded"), "{error:#}");
    }

    #[test]
    fn rejects_unsorted_and_padded_trigger_tokens() {
        let mut doc = valid_doc();
        doc.authority_exception[0].trigger = "workflow_dispatch,schedule".to_string();
        let error = validate_doc(&doc, today(), |_| true).expect_err("unsorted must fail");
        assert!(error.to_string().contains("sorted unique"), "{error:#}");

        // Padding is rejected on its own terms. `check-workflow-surfaces`
        // compares the trigger byte for byte, so a padded token could never
        // match its finding; the error must name the padding, not sorting.
        let mut padded = valid_doc();
        padded.authority_exception[0].trigger = "schedule, workflow_dispatch".to_string();
        let error = validate_doc(&padded, today(), |_| true).expect_err("padding must fail");
        assert!(error.to_string().contains("whitespace"), "{error:#}");
    }

    #[test]
    fn rejects_schema_policy_and_status_mismatches() {
        let mut schema = valid_doc();
        schema.schema_version = "2.0".to_string();
        assert!(validate_doc(&schema, today(), |_| true).is_err());

        let mut policy = valid_doc();
        policy.policy = "something-else".to_string();
        assert!(validate_doc(&policy, today(), |_| true).is_err());

        let mut status = valid_doc();
        status.status = "draft".to_string();
        assert!(validate_doc(&status, today(), |_| true).is_err());
    }

    #[test]
    fn reports_every_invalid_entry_rather_than_only_the_first() {
        let mut doc = valid_doc();
        let mut second = doc.authority_exception[0].clone();
        second.job = "another-job".to_string();
        second.repository = "Attacker/evil".to_string();
        doc.authority_exception[0].capability = "contents:*".to_string();
        doc.authority_exception.push(second);

        let error = validate_doc(&doc, today(), |_| true).expect_err("both entries must fail");
        let rendered = format!("{error:#}");
        assert!(rendered.contains("wildcarded"), "{rendered}");
        assert!(rendered.contains("EffortlessMetrics"), "{rendered}");
    }
}
