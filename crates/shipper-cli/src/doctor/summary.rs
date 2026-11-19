//! Readiness summary shared by human and JSON `shipper doctor` output.

use serde::Serialize;

use crate::output::outcome::{ActionKind, OperatorAction};

use super::findings::{Finding, FindingLevel};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DoctorReadiness {
    Ready,
    ReadyWithWarnings,
    Blocked,
    Incomplete,
}

impl DoctorReadiness {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::ReadyWithWarnings => "ready_with_warnings",
            Self::Blocked => "blocked",
            Self::Incomplete => "incomplete",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DoctorCheckStatus {
    Passed,
    Warning,
    Blocked,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DoctorSummary {
    pub checks_evaluated: usize,
    pub checks_passed: usize,
    pub warning_count: usize,
    pub blocker_count: usize,
    pub skipped_or_unknown_count: usize,
    /// Names of the checks that could not be evaluated. A bare count leaves the
    /// operator with no way to act on it, and `--format json` carried the same
    /// count without the names, so the "investigate unknowns" action pointed at
    /// output that could not answer the question either.
    pub unknown_checks: Vec<&'static str>,
    pub readiness: DoctorReadiness,
    pub next_action: OperatorAction,
}

impl DoctorSummary {
    pub(super) fn from_checks(
        checks: impl IntoIterator<Item = (&'static str, DoctorCheckStatus)>,
    ) -> Self {
        let mut checks_evaluated = 0;
        let mut checks_passed = 0;
        let mut warning_count = 0;
        let mut blocker_count = 0;
        let mut unknown_checks = Vec::new();

        for (name, status) in checks {
            checks_evaluated += 1;
            match status {
                DoctorCheckStatus::Passed => checks_passed += 1,
                DoctorCheckStatus::Warning => warning_count += 1,
                DoctorCheckStatus::Blocked => blocker_count += 1,
                DoctorCheckStatus::Unknown => unknown_checks.push(name),
            }
        }

        Self::from_counts(
            checks_evaluated,
            checks_passed,
            warning_count,
            blocker_count,
            unknown_checks,
        )
    }

    pub(super) fn combine(
        workspace: DoctorCheckStatus,
        reports: impl IntoIterator<Item = DoctorSummary>,
    ) -> Self {
        let mut combined = Self::from_checks([("workspace", workspace)]);
        for report in reports {
            combined.checks_evaluated += report.checks_evaluated;
            combined.checks_passed += report.checks_passed;
            combined.warning_count += report.warning_count;
            combined.blocker_count += report.blocker_count;
            combined.unknown_checks.extend(report.unknown_checks);
        }
        Self::from_counts(
            combined.checks_evaluated,
            combined.checks_passed,
            combined.warning_count,
            combined.blocker_count,
            combined.unknown_checks,
        )
    }

    fn from_counts(
        checks_evaluated: usize,
        checks_passed: usize,
        warning_count: usize,
        blocker_count: usize,
        unknown_checks: Vec<&'static str>,
    ) -> Self {
        let skipped_or_unknown_count = unknown_checks.len();
        let readiness = if blocker_count > 0 {
            DoctorReadiness::Blocked
        } else if skipped_or_unknown_count > 0 {
            DoctorReadiness::Incomplete
        } else if warning_count > 0 {
            DoctorReadiness::ReadyWithWarnings
        } else {
            DoctorReadiness::Ready
        };
        let next_action = match readiness {
            DoctorReadiness::Ready => OperatorAction::command(
                ActionKind::Plan,
                ["shipper", "plan"],
                "the required environment checks passed",
            ),
            DoctorReadiness::ReadyWithWarnings => OperatorAction::command(
                ActionKind::Plan,
                ["shipper", "plan"],
                "no blocker remains; review warnings before preflight",
            ),
            DoctorReadiness::Blocked => OperatorAction::command(
                ActionKind::ResolveBlockers,
                ["shipper", "doctor"],
                "apply the blocking finding remediation, then rerun diagnostics",
            ),
            DoctorReadiness::Incomplete => OperatorAction::command(
                ActionKind::InvestigateUnknowns,
                ["shipper", "doctor", "--format", "json"],
                "the checks named under Unknown could not be evaluated; resolve them, then rerun diagnostics",
            ),
        };

        Self {
            checks_evaluated,
            checks_passed,
            warning_count,
            blocker_count,
            skipped_or_unknown_count,
            unknown_checks,
            readiness,
            next_action,
        }
    }

    pub(super) fn print_human(&self) {
        self.print_human_with_label("Doctor");
    }

    pub(super) fn print_registry_human(&self) {
        self.print_human_with_label("Registry doctor");
    }

    fn print_human_with_label(&self, label: &str) {
        println!();
        println!("{label}: {}", self.readiness.as_str());
        println!(
            "Checks: {} evaluated, {} passed, {}, {}, {} unknown",
            self.checks_evaluated,
            self.checks_passed,
            plural(self.warning_count, "warning"),
            plural(self.blocker_count, "blocker"),
            self.skipped_or_unknown_count
        );
        if !self.unknown_checks.is_empty() {
            println!("Unknown: {}", self.unknown_checks.join(", "));
        }
        // The command alone is often circular ("Next: shipper doctor" after a
        // `shipper doctor` run). The reason is what makes the line actionable,
        // and it was already being emitted to JSON but withheld from humans.
        match self.next_action.command_line() {
            Some(command) => println!("Next: {command} — {}", self.next_action.reason),
            None => println!("Next: {}", self.next_action.reason),
        }
    }
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

pub(super) fn status_from_findings(findings: &[Finding]) -> DoctorCheckStatus {
    if findings
        .iter()
        .any(|finding| finding.status == FindingLevel::Blocked)
    {
        DoctorCheckStatus::Blocked
    } else if findings
        .iter()
        .any(|finding| finding.status == FindingLevel::Unknown)
    {
        DoctorCheckStatus::Unknown
    } else if findings
        .iter()
        .any(|finding| finding.status == FindingLevel::Warning)
    {
        DoctorCheckStatus::Warning
    } else {
        DoctorCheckStatus::Passed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_is_ready_when_every_check_passes() {
        let summary = DoctorSummary::from_checks([
            ("registry auth", DoctorCheckStatus::Passed),
            ("git context", DoctorCheckStatus::Passed),
        ]);
        assert_eq!(summary.readiness, DoctorReadiness::Ready);
        assert_eq!(summary.checks_evaluated, 2);
        assert_eq!(summary.checks_passed, 2);
        assert_eq!(summary.next_action.kind, ActionKind::Plan);
    }

    #[test]
    fn blockers_take_precedence_over_warnings_and_unknowns() {
        let summary = DoctorSummary::from_checks([
            ("encryption", DoctorCheckStatus::Warning),
            ("git context", DoctorCheckStatus::Unknown),
            ("registry auth", DoctorCheckStatus::Blocked),
        ]);
        assert_eq!(summary.readiness, DoctorReadiness::Blocked);
        assert_eq!(summary.warning_count, 1);
        assert_eq!(summary.blocker_count, 1);
        assert_eq!(summary.skipped_or_unknown_count, 1);
    }

    #[test]
    fn unknown_required_checks_make_the_result_incomplete() {
        let summary = DoctorSummary::from_checks([
            ("registry auth", DoctorCheckStatus::Passed),
            ("git context", DoctorCheckStatus::Unknown),
        ]);
        assert_eq!(summary.readiness, DoctorReadiness::Incomplete);
        assert_eq!(summary.next_action.kind, ActionKind::InvestigateUnknowns);
        // An unattributed count is not actionable: name what could not be
        // evaluated so the operator has somewhere to go.
        assert_eq!(summary.unknown_checks, vec!["git context"]);
        assert_eq!(
            summary.skipped_or_unknown_count,
            summary.unknown_checks.len()
        );
    }

    #[test]
    fn combined_summaries_keep_every_unknown_check_name() {
        let registry_a = DoctorSummary::from_checks([
            ("registry auth", DoctorCheckStatus::Passed),
            ("git context", DoctorCheckStatus::Unknown),
        ]);
        let registry_b = DoctorSummary::from_checks([("encryption", DoctorCheckStatus::Unknown)]);

        let combined = DoctorSummary::combine(DoctorCheckStatus::Passed, [registry_a, registry_b]);

        assert_eq!(combined.unknown_checks, vec!["git context", "encryption"]);
        assert_eq!(combined.skipped_or_unknown_count, 2);
        assert_eq!(combined.checks_evaluated, 4);
        assert_eq!(combined.readiness, DoctorReadiness::Incomplete);
    }

    #[test]
    fn single_counts_read_as_singular() {
        assert_eq!(plural(1, "blocker"), "1 blocker");
        assert_eq!(plural(0, "blocker"), "0 blockers");
        assert_eq!(plural(2, "warning"), "2 warnings");
    }

    #[test]
    fn warnings_without_blockers_remain_actionable() {
        let summary = DoctorSummary::from_checks([
            ("registry auth", DoctorCheckStatus::Passed),
            ("encryption", DoctorCheckStatus::Warning),
        ]);
        assert_eq!(summary.readiness, DoctorReadiness::ReadyWithWarnings);
        assert_eq!(
            summary.next_action.command,
            vec!["shipper".to_string(), "plan".to_string()]
        );
    }
}
