//! Observe state-path type and effective access without changing retained evidence.

use std::fs;
use std::io;
use std::path::Path;

use shipper_core::plan;
use shipper_core::types::RuntimeOptions;

use crate::doctor::findings::{Finding, FindingLevel};

mod probe;

#[derive(Debug, serde::Serialize)]
pub(in crate::doctor) struct StateDirCheck {
    pub path: String,
    /// Omitted when inspection could not establish existence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
    /// Effective probe result, never inferred from permission mode bits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub writable: Option<bool>,
    pub observation: StateDirObservation,
    pub findings: Vec<Finding>,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(in crate::doctor) enum StateDirObservation {
    WritableDirectory,
    NotDirectory {
        file_type: &'static str,
    },
    ParentNotDirectory {
        #[serde(skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    Missing {
        existing_parent: String,
    },
    UnresolvedSymlink {
        path: String,
    },
    ChangedDuringInspection {
        path: String,
    },
    InspectionFailed {
        path: String,
        error_kind: String,
    },
    ProbeFailed {
        operation: probe::Operation,
        error_kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cleanup_error_kind: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        retained_probe: Option<String>,
    },
}

pub(in crate::doctor) fn check(
    ws: &plan::PlannedWorkspace,
    opts: &RuntimeOptions,
) -> StateDirCheck {
    let check = inspect(ws, opts);
    println!("state_dir: {}", check.path);
    match check.exists {
        Some(false) if matches!(check.observation, StateDirObservation::Missing { .. }) => {
            println!("state_dir_exists: false (prospective; write access is unverified)");
        }
        Some(false) => println!("state_dir_exists: false"),
        None => println!("state_dir_exists: unknown (inspection was inconclusive)"),
        Some(true) => {}
    }
    if let Some(writable) = check.writable {
        println!("state_dir_writable: {writable}");
    }
    check
}

pub(in crate::doctor) fn inspect(
    ws: &plan::PlannedWorkspace,
    opts: &RuntimeOptions,
) -> StateDirCheck {
    let abs_state = if opts.state_dir.is_absolute() {
        opts.state_dir.clone()
    } else {
        ws.workspace_root.join(&opts.state_dir)
    };
    let (exists, observation) = observe(&abs_state);
    let writable = match &observation {
        StateDirObservation::WritableDirectory => Some(true),
        StateDirObservation::ProbeFailed { operation, .. }
            if *operation != probe::Operation::Remove =>
        {
            Some(false)
        }
        _ => None,
    };
    let findings = finding(&abs_state, &observation).into_iter().collect();
    StateDirCheck {
        path: abs_state.display().to_string(),
        exists,
        writable,
        observation,
        findings,
    }
}

fn observe(path: &Path) -> (Option<bool>, StateDirObservation) {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            let observation = match probe::directory(path) {
                Ok(()) => StateDirObservation::WritableDirectory,
                Err(error) => StateDirObservation::ProbeFailed {
                    operation: error.operation,
                    error_kind: format!("{:?}", error.kind),
                    cleanup_error_kind: error.cleanup_kind.map(|kind| format!("{kind:?}")),
                    retained_probe: error.retained_path.map(|path| path.display().to_string()),
                },
            };
            (Some(true), observation)
        }
        Ok(metadata) => (
            Some(true),
            StateDirObservation::NotDirectory {
                file_type: if metadata.is_file() {
                    "regular_file"
                } else {
                    "other"
                },
            },
        ),
        Err(error) if unresolved_path(error.kind()) => inspect_ancestors(path, error.kind()),
        Err(error) => inspection_failed(path, error.kind()),
    }
}

fn unresolved_path(kind: io::ErrorKind) -> bool {
    matches!(kind, io::ErrorKind::NotFound | io::ErrorKind::NotADirectory)
}

fn inspection_failed(path: &Path, kind: io::ErrorKind) -> (Option<bool>, StateDirObservation) {
    (
        None,
        StateDirObservation::InspectionFailed {
            path: path.display().to_string(),
            error_kind: format!("{kind:?}"),
        },
    )
}

fn changed_path(path: &Path) -> (Option<bool>, StateDirObservation) {
    (
        None,
        StateDirObservation::ChangedDuringInspection {
            path: path.display().to_string(),
        },
    )
}

fn inspect_ancestors(
    path: &Path,
    original_error: io::ErrorKind,
) -> (Option<bool>, StateDirObservation) {
    for candidate in path.ancestors() {
        match fs::metadata(candidate) {
            Ok(_) if candidate == path => {
                // A successful second observation does not establish access to
                // the path that was missing or unresolved on the first read.
                return changed_path(path);
            }
            Ok(metadata) if !metadata.is_dir() => {
                return (
                    Some(false),
                    StateDirObservation::ParentNotDirectory {
                        parent: Some(candidate.display().to_string()),
                    },
                );
            }
            Ok(_) if original_error == io::ErrorKind::NotADirectory => {
                // Some platforms identify a non-directory component without
                // identifying which component (e.g. a trailing slash on a file).
                return (
                    Some(false),
                    StateDirObservation::ParentNotDirectory { parent: None },
                );
            }
            Ok(_) => {
                return (
                    Some(false),
                    StateDirObservation::Missing {
                        existing_parent: candidate.display().to_string(),
                    },
                );
            }
            Err(error) if unresolved_path(error.kind()) => match fs::symlink_metadata(candidate) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return (
                        Some(candidate == path),
                        StateDirObservation::UnresolvedSymlink {
                            path: candidate.display().to_string(),
                        },
                    );
                }
                Ok(_) => return changed_path(candidate),
                Err(error) if unresolved_path(error.kind()) => {}
                Err(error) => return inspection_failed(candidate, error.kind()),
            },
            Err(error) => return inspection_failed(candidate, error.kind()),
        }
    }
    inspection_failed(path, original_error)
}

fn finding(path: &Path, observation: &StateDirObservation) -> Option<Finding> {
    use StateDirObservation::*;

    let (id, status, title, reason, try_next) = match observation {
        WritableDirectory => return None,
        NotDirectory { file_type } => (
            "state-dir-not-directory", FindingLevel::Blocked, "state path is not a directory",
            format!("observed path type: {file_type}; no access probe was attempted"),
            vec!["preserve the existing file and inspect it before changing this path", "choose a directory with `--state-dir <path>`", "rerun `shipper doctor`"],
        ),
        ParentNotDirectory { parent } => (
            "state-dir-parent-not-directory", FindingLevel::Blocked, "state path crosses a non-directory component",
            parent.as_ref().map_or_else(|| "metadata reported a non-directory path component; no access probe was attempted".to_string(), |parent| format!("blocking path: {parent}; no access probe was attempted")),
            vec!["preserve and inspect the blocking path", "choose a state path beneath an existing directory", "rerun `shipper doctor`"],
        ),
        Missing { existing_parent } => (
            "state-dir-not-created", FindingLevel::Unknown, "state directory is prospective; write access is unverified",
            format!("path is absent; existing parent: {existing_parent}; no directory or access probe was created"),
            vec!["create the intended directory or select an existing directory with `--state-dir <path>`", "rerun `shipper doctor` to check effective access"],
        ),
        UnresolvedSymlink { path } => (
            "state-dir-unresolved-symlink", FindingLevel::Blocked, "state path contains an unresolved symbolic link",
            format!("unresolved link: {path}; the link was preserved"),
            vec!["inspect the link and its intended target without deleting retained evidence", "select an existing directory with `--state-dir <path>`", "rerun `shipper doctor`"],
        ),
        ChangedDuringInspection { path } => (
            "state-dir-inspection-changed", FindingLevel::Unknown, "state path changed during inspection",
            format!("different path observations at {path}; existence and write access are unverified"),
            vec!["wait for other changes to this path to finish", "rerun `shipper doctor` to obtain a consistent observation"],
        ),
        InspectionFailed { path, error_kind } => (
            "state-dir-inspection-failed", FindingLevel::Unknown, "state path could not be inspected",
            format!("metadata observation failed at {path}: {error_kind}; existence and write access are unverified"),
            vec!["check path visibility and access permissions for this user", "rerun `shipper doctor` after resolving the inspection error"],
        ),
        ProbeFailed { operation, error_kind, cleanup_error_kind, retained_probe } => {
            let mut reason = format!("directory probe failed during {}: {error_kind}", operation.as_str());
            if let Some(error) = cleanup_error_kind {
                reason.push_str(&format!("; probe cleanup also failed: {error}"));
            }
            if let Some(probe) = retained_probe {
                reason.push_str(&format!("; owned probe may remain at {probe}"));
            }
            (
                "state-dir-write-probe-failed", FindingLevel::Blocked, "state directory access probe failed", reason,
                vec!["check filesystem access, capacity, and policy for this user", "preserve existing evidence; inspect any named leftover probe before removing it", "choose a usable directory with `--state-dir <path>` and rerun `shipper doctor`"],
            )
        }
    };
    Some(Finding {
        id,
        severity: if status == FindingLevel::Unknown {
            FindingLevel::Warning
        } else {
            status
        },
        status,
        title,
        why_it_matters: "publish and resume need a usable directory for state, events, receipts, and locks; a diagnostic does not guarantee future access or durability",
        evidence: format!("state_dir: {}; {reason}", path.display()),
        try_next,
        docs: Some("docs/reference/state-files.md"),
    })
}
