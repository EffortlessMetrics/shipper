//! Per-package upload evidence shared by admission, execution, and replay.

use crate::types::{
    AttemptDetail, ErrorClass, EventType, PackageProgress, PackageState, ReconciliationOutcome,
};

#[derive(Clone, Copy, Default)]
pub(crate) enum UploadWitness {
    #[default]
    None,
    Attempted {
        number: u32,
        legacy: bool,
        output_recorded: bool,
    },
    Unconfirmed(Option<u32>),
    NotLaunched(u32),
    Uploaded,
    LegacyUploaded(u32),
    LegacyVisibilityFailed(u32),
    Terminal,
}

impl UploadWitness {
    pub(crate) fn observe(self, event: &EventType) -> Self {
        match (self, event) {
            (
                _,
                EventType::PackagePublished { .. }
                | EventType::PackageSkipped { .. }
                | EventType::PublishReconciled {
                    outcome: ReconciliationOutcome::Published { .. },
                },
            ) => Self::Terminal,
            (Self::Terminal, _) => self,
            (_, EventType::PackageUploaded) => Self::Uploaded,
            (
                Self::LegacyUploaded(attempt),
                EventType::PackageFailed {
                    class: ErrorClass::Ambiguous,
                    ..
                },
            ) => Self::LegacyVisibilityFailed(attempt),
            (Self::Uploaded | Self::LegacyUploaded(_) | Self::LegacyVisibilityFailed(_), _) => self,
            (
                _,
                EventType::PackageAttempted {
                    attempt,
                    max_attempts,
                    ..
                },
            ) => Self::Attempted {
                number: *attempt,
                legacy: *max_attempts == 0,
                output_recorded: false,
            },
            (Self::Attempted { number, legacy, .. }, EventType::PackageOutput { .. }) => {
                Self::Attempted {
                    number,
                    legacy,
                    output_recorded: true,
                }
            }
            (
                Self::Attempted {
                    number,
                    legacy: false,
                    output_recorded: false,
                },
                EventType::PackageNotLaunched { attempt, .. },
            ) if number == *attempt => Self::NotLaunched(number),
            // v0.4 saved Uploaded after writing Cargo output and emitted
            // readiness before polling. Modern readiness diagnostics alone
            // cannot establish that Cargo accepted an upload.
            (
                Self::Attempted {
                    number,
                    legacy: true,
                    output_recorded: true,
                },
                EventType::ReadinessStarted { .. },
            ) => Self::LegacyUploaded(number),
            (
                _,
                EventType::PackageUploadUnconfirmed { .. }
                | EventType::PackageFailed {
                    class: ErrorClass::Ambiguous,
                    ..
                }
                | EventType::PublishReconciled {
                    outcome: ReconciliationOutcome::StillUnknown { .. },
                },
            ) => Self::Unconfirmed(match self {
                Self::Attempted { number, .. } => Some(number),
                Self::Unconfirmed(attempt) => attempt,
                _ => None,
            }),
            (
                _,
                EventType::PackageFailed { .. }
                | EventType::PublishReconciled {
                    outcome: ReconciliationOutcome::NotPublished { .. },
                },
            ) => Self::None,
            _ => self,
        }
    }

    /// v0.4 could finish a successful Cargo attempt with a package-level
    /// ambiguous failure when readiness ran out. Only visibility can resolve
    /// that retained state. A negative probe cannot authorize another upload.
    pub(crate) fn requires_legacy_visibility_check(self, progress: &PackageProgress) -> bool {
        matches!(self, Self::LegacyVisibilityFailed(attempt) if attempt == progress.attempts)
            && matches!(
                progress.state,
                PackageState::Failed {
                    class: ErrorClass::Ambiguous,
                    ..
                }
            )
    }

    pub(crate) fn permits(
        self,
        progress: Option<&PackageProgress>,
        attempt_history: &[AttemptDetail],
    ) -> bool {
        if matches!(self, Self::None) {
            return true;
        }
        let Some(progress) = progress else {
            return false;
        };
        // Authentic old terminal snapshots already skip, including in
        // histories without a corresponding terminal event.
        if matches!(
            progress.state,
            PackageState::Published | PackageState::Skipped { .. }
        ) {
            return true;
        }
        match self {
            Self::Unconfirmed(attempt) => {
                attempt.is_none_or(|number| number == progress.attempts)
                    && matches!(
                        progress.state,
                        PackageState::Ambiguous { .. }
                            | PackageState::Failed {
                                class: ErrorClass::Ambiguous,
                                ..
                            }
                    )
            }
            Self::NotLaunched(attempt) => {
                let PackageState::Failed {
                    class: ErrorClass::Retryable,
                    message,
                } = &progress.state
                else {
                    return false;
                };
                attempt == progress.attempts
                    && attempt_history.iter().any(|detail| {
                        detail.package == progress.name
                            && detail.version == progress.version
                            && detail.attempt == attempt
                            && detail.error_class == Some(ErrorClass::Retryable)
                            && detail.next_attempt_at.is_none()
                            && detail.redacted_message.as_ref() == Some(message)
                    })
            }
            Self::Uploaded => progress.state == PackageState::Uploaded,
            Self::LegacyUploaded(attempt) => {
                attempt == progress.attempts && progress.state == PackageState::Uploaded
            }
            Self::LegacyVisibilityFailed(_) => self.requires_legacy_visibility_check(progress),
            // An authentic v0.4 snapshot could commit immediately before
            // ReadinessStarted. It can still verify visibility safely.
            Self::Attempted {
                number,
                legacy: true,
                output_recorded: true,
            } => number == progress.attempts && progress.state == PackageState::Uploaded,
            _ => false,
        }
    }
}
