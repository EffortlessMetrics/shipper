//! Ambiguous-publish reconciliation against registry truth.
//!
//! `cargo publish` uploads a crate to the registry first, then polls the
//! index to confirm visibility. The poll can time out (or the cargo process
//! can crash) **without affecting the upload**. This means a non-zero cargo
//! exit can coexist with a successful upload — the classic ambiguous case.
//!
//! Blind-retrying in that state risks a duplicate-upload attempt that the
//! registry will reject (or worse, it risks re-uploading if the window has
//! cleared). The safe move is to **reconcile against registry truth**: poll
//! the sparse index or API with bounded patience, and resolve one of three
//! outcomes:
//!
//! - **Published** — the version appeared; treat as success, do not retry.
//! - **NotPublished** — within the budget, the version was never visible;
//!   the caller may safely enter the normal retry path (no duplicate risk).
//! - **StillUnknown** — queries themselves kept failing; the caller MUST
//!   NOT retry, and MUST mark the package state Ambiguous for operator
//!   decision.
//!
//! This module wires the existing [`super::readiness::is_version_visible_with_backoff`]
//! polling loop into that decision, translating its return into a
//! [`ReconciliationOutcome`]. See [issue #99](https://github.com/EffortlessMetrics/shipper/issues/99)
//! for the full design discussion.

use std::time::Instant;

use shipper_registry::HttpRegistryClient as RegistryClient;
use shipper_types::{ReadinessConfig, ReadinessEvidence, ReconciliationOutcome};

use super::readiness::is_version_visible_with_backoff;

/// Reconcile an ambiguous `cargo publish` outcome against registry truth.
///
/// Polls the registry (sparse index + API per `config.method`) with bounded
/// patience. Returns a [`ReconciliationOutcome`] classifying the real state
/// plus the accumulated [`ReadinessEvidence`] so the caller can attach it to
/// the package receipt for audit.
///
/// Callers (the retry loop in [`super::publish`]) MUST honor the outcome:
/// - `Published` → advance; no further retry of `cargo publish` for this crate.
/// - `NotPublished` → it's safe to enter the normal retry path.
/// - `StillUnknown` → do not retry; escalate to operator (mark the package
///   state `Ambiguous` and halt).
pub(super) fn reconcile_ambiguous_upload(
    reg: &RegistryClient,
    crate_name: &str,
    version: &str,
    config: &ReadinessConfig,
) -> (ReconciliationOutcome, Vec<ReadinessEvidence>) {
    let start = Instant::now();

    match is_version_visible_with_backoff(reg, crate_name, version, config) {
        Ok((true, evidence)) => (
            ReconciliationOutcome::Published {
                attempts: evidence.len() as u32,
                elapsed_ms: start.elapsed().as_millis() as u64,
            },
            evidence,
        ),
        Ok((false, evidence)) => (
            ReconciliationOutcome::NotPublished {
                attempts: evidence.len() as u32,
                elapsed_ms: start.elapsed().as_millis() as u64,
            },
            evidence,
        ),
        Err(e) => (
            ReconciliationOutcome::StillUnknown {
                attempts: 0,
                elapsed_ms: start.elapsed().as_millis() as u64,
                reason: format!("reconciliation query failed: {e}"),
            },
            Vec::new(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Unit smoke-tests for ReconciliationOutcome variants. Integration tests
    // with a live `tiny_http` mock live in the parallel engine's test suite
    // where the full publish pipeline can be exercised end-to-end.

    #[test]
    fn published_outcome_carries_attempts_and_elapsed() {
        let outcome = ReconciliationOutcome::Published {
            attempts: 3,
            elapsed_ms: 1500,
        };
        match outcome {
            ReconciliationOutcome::Published {
                attempts,
                elapsed_ms,
            } => {
                assert_eq!(attempts, 3);
                assert_eq!(elapsed_ms, 1500);
            }
            _ => panic!("expected Published"),
        }
    }

    #[test]
    fn still_unknown_carries_reason() {
        let outcome = ReconciliationOutcome::StillUnknown {
            attempts: 0,
            elapsed_ms: 42,
            reason: "query failed".to_string(),
        };
        match outcome {
            ReconciliationOutcome::StillUnknown { reason, .. } => {
                assert_eq!(reason, "query failed");
            }
            _ => panic!("expected StillUnknown"),
        }
    }
}
