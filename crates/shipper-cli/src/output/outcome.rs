//! Shared semantic vocabulary for operator-facing command guidance.
//!
//! Human prose and versioned JSON envelopes may render these values
//! differently, but they should not independently invent the next safe command.
//! Engine/state/receipt truth remains authoritative; this module is the CLI
//! adaptation layer for that truth.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[expect(
    dead_code,
    reason = "#274 integrates action kinds across primary commands in sequential PRs"
)]
pub(crate) enum ActionKind {
    Plan,
    Preflight,
    Publish,
    Resume,
    Status,
    InspectEvents,
    InspectReceipt,
    Reconcile,
    FixConfiguration,
    WaitForRegistry,
    ResolveBlockers,
    InvestigateUnknowns,
    StopAndInvestigate,
    NoneComplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OperatorAction {
    pub kind: ActionKind,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    pub reason: String,
    pub requires_confirmation: bool,
}

impl OperatorAction {
    pub(crate) fn command<I, S, R>(kind: ActionKind, command: I, reason: R) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
        R: Into<String>,
    {
        Self {
            kind,
            command: command.into_iter().map(Into::into).collect(),
            reason: reason.into(),
            requires_confirmation: false,
        }
    }

    pub(crate) fn posture(kind: ActionKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            command: Vec::new(),
            reason: reason.into(),
            requires_confirmation: false,
        }
    }

    pub(crate) fn with_confirmation(mut self) -> Self {
        self.requires_confirmation = true;
        self
    }

    pub(crate) fn command_line(&self) -> Option<String> {
        (!self.command.is_empty()).then(|| self.command.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_action_has_stable_machine_and_human_identity() {
        let action = OperatorAction::command(
            ActionKind::Resume,
            ["shipper", "resume"],
            "durable evidence proves the interrupted run is resumable",
        );
        assert_eq!(action.command_line().as_deref(), Some("shipper resume"));
        assert!(!action.requires_confirmation);
        let json = serde_json::to_value(&action).expect("serialize action");
        assert_eq!(json["kind"], "resume");
        assert_eq!(json["command"], serde_json::json!(["shipper", "resume"]));
    }

    #[test]
    fn stop_posture_does_not_fabricate_a_command() {
        let action = OperatorAction::posture(
            ActionKind::StopAndInvestigate,
            "registry outcome remains unknown",
        );
        assert_eq!(action.command_line(), None);
        assert_eq!(
            serde_json::to_value(&action).expect("serialize action")["kind"],
            "stop_and_investigate"
        );
    }

    #[test]
    fn confirmation_is_explicit_not_inferred_from_action_kind() {
        let action = OperatorAction::command(
            ActionKind::Publish,
            ["shipper", "publish"],
            "reversible proof passed",
        )
        .with_confirmation();
        assert!(action.requires_confirmation);
    }
}
