//! Diagnostic finding records and rendering for `shipper doctor`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FindingLevel {
    Blocked,
    Warning,
}

impl FindingLevel {
    fn as_str(self) -> &'static str {
        match self {
            FindingLevel::Blocked => "blocked",
            FindingLevel::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Finding {
    pub id: &'static str,
    pub severity: FindingLevel,
    pub status: FindingLevel,
    pub title: &'static str,
    pub why_it_matters: &'static str,
    pub evidence: String,
    pub try_next: Vec<&'static str>,
    pub docs: Option<&'static str>,
}

pub(super) fn print_findings(findings: &[Finding]) {
    println!();
    println!("Findings:");
    println!("---------");
    if findings.is_empty() {
        println!("  none");
        return;
    }

    for finding in findings {
        println!(
            "  [{}] {} ({})",
            finding.status.as_str(),
            finding.title,
            finding.id
        );
        println!("    status: {}", finding.status.as_str());
        println!("    severity: {}", finding.severity.as_str());
        println!("    why: {}", finding.why_it_matters);
        println!("    evidence: {}", finding.evidence);
        println!("    try next:");
        for step in &finding.try_next {
            println!("      - {step}");
        }
        if let Some(docs) = finding.docs {
            println!("    docs: {docs}");
        }
    }
}
