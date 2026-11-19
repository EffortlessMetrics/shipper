//! Resolve the existing plan and path owners into one containment context.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use shipper_core::plan::PlannedWorkspace;
use shipper_core::runtime::execution::resolve_state_dir;
use shipper_core::state::events::{EventLog, events_path};
use shipper_core::state::execution_state::{load_receipt, receipt_path, write_receipt};
use shipper_core::types::{EventType, PublishEvent, RuntimeOptions};

pub(super) struct ContainmentContext<'a> {
    pub workspace_root: &'a Path,
    pub registry_name: &'a str,
    pub state_dir: PathBuf,
}

impl<'a> ContainmentContext<'a> {
    pub fn resolve(planned: &'a PlannedWorkspace, opts: &RuntimeOptions) -> Result<Self> {
        // The CLI rejects multi-registry selectors before planning. Keep this
        // boundary explicit too: containment never chooses the first target.
        if !opts.registries.is_empty() {
            bail!("containment supports one registry; select it with --registry");
        }
        let registry_name = planned.plan.registry.name.as_str();
        if registry_name.trim().is_empty() {
            bail!("containment requires a non-empty configured registry name");
        }
        Ok(Self {
            workspace_root: &planned.workspace_root,
            registry_name,
            state_dir: resolve_state_dir(&planned.workspace_root, &opts.state_dir),
        })
    }

    pub fn validate_plan_registry(&self, kind: &str, registry: &str) -> Result<()> {
        if registry != self.registry_name {
            bail!(
                "{kind} registry '{registry}' does not match configured registry '{}'",
                self.registry_name
            );
        }
        Ok(())
    }

    pub fn prepare_events(&self) -> Result<()> {
        let path = events_path(&self.state_dir);
        // Use the existing append owner with an empty batch. This prepares the
        // destination without inventing a pre-effect event or replacing bytes.
        // A later append must still be checked: preparation cannot prevent races.
        EventLog::new().write_to_file(&path).map_err(|_| {
            anyhow!(
                "cannot prepare containment event evidence at {}; no Cargo yank was started. \
                 Repair the evidence destination before containment",
                path.display()
            )
        })
    }

    pub fn prepare_receipt_amendment(&self, name: &str, version: &str) -> Result<()> {
        let path = receipt_path(&self.state_dir);
        let refused = |detail| {
            anyhow!(
                "cannot prepare requested --mark-compromised receipt amendment for \
                 {name}@{version} at {}: {detail}; no Cargo yank was started. \
                 Repair the retained receipt before containment",
                path.display()
            )
        };
        let receipt = load_receipt(&self.state_dir)
            .map_err(|_| refused("receipt could not be read"))?
            .ok_or_else(|| refused("receipt is missing"))?;
        if receipt.registry.name != self.registry_name {
            return Err(refused(
                "receipt registry does not match the selected registry",
            ));
        }
        if !receipt
            .packages
            .iter()
            .any(|package| package.name == name && package.version == version)
        {
            return Err(refused("receipt has no matching package entry"));
        }
        Ok(())
    }

    pub fn record_yank(
        &self,
        name: &str,
        version: &str,
        reason: &str,
        exit_code: i32,
    ) -> Result<()> {
        let path = events_path(&self.state_dir);
        let mut log = EventLog::new();
        log.record(PublishEvent {
            timestamp: chrono::Utc::now(),
            event_type: EventType::PackageYanked {
                crate_name: name.to_string(),
                version: version.to_string(),
                reason: reason.to_string(),
                exit_code,
            },
            package: format!("{name}@{version}"),
        });
        log.write_to_file(&path).map_err(|_| {
            let observed = if exit_code == 0 {
                "Cargo reported success (exit 0)".to_string()
            } else {
                format!("Cargo exited with code {exit_code}")
            };
            // Retain the observed result without echoing Cargo output, operator
            // reason text, or parser diagnostics into a new essential error.
            anyhow!(
                "containment evidence incomplete: {observed} for {name}@{version}, \
                 but persistence of its PackageYanked event could not be confirmed at {}. \
                 Remaining containment actions stopped. Reconcile registry truth and \
                 retained evidence before any further yank; do not retry automatically",
                path.display()
            )
        })
    }

    pub fn mark_compromised(&self, name: &str, version: &str, reason: &str) -> Result<PathBuf> {
        let path = receipt_path(&self.state_dir);
        let incomplete = |detail| {
            anyhow!(
                "containment evidence incomplete: Cargo reported success (exit 0) for \
                 {name}@{version} and its PackageYanked event was recorded at {}, but \
                 the requested --mark-compromised receipt amendment failed at {}: {detail}. \
                 Reconcile registry truth and repair the retained receipt; do not repeat \
                 the yank to retry this amendment",
                events_path(&self.state_dir).display(),
                path.display()
            )
        };
        // Re-read after the effect. The receipt may have changed since preflight;
        // an old in-memory snapshot must not silently overwrite those changes.
        let mut receipt = load_receipt(&self.state_dir)
            .map_err(|_| incomplete("receipt could not be read"))?
            .ok_or_else(|| incomplete("receipt is missing"))?;
        if receipt.registry.name != self.registry_name {
            return Err(incomplete(
                "receipt registry does not match the selected registry",
            ));
        }
        let package = receipt
            .packages
            .iter_mut()
            .find(|package| package.name == name && package.version == version)
            .ok_or_else(|| incomplete("receipt has no matching package entry"))?;
        package.compromised_at = Some(chrono::Utc::now());
        package.compromised_by = Some(reason.to_string());
        write_receipt(&self.state_dir, &receipt)
            .map_err(|_| incomplete("receipt could not be written"))?;
        Ok(path)
    }
}
