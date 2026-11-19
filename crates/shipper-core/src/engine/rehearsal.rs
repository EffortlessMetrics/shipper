use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::engine::Reporter;
use crate::plan::PlannedWorkspace;
use crate::types::RuntimeOptions;

/// Enforce the rehearsal hard gate (#97 PR 3).
///
/// Rules, evaluated in order:
///
/// 1. **Rehearsal not configured** (`opts.rehearsal_registry` is `None`) →
///    gate is dormant; publish proceeds. Rehearsal is opt-in; existing
///    workflows that never set a rehearsal registry are unaffected.
///
/// 2. **Operator override** (`opts.rehearsal_skip` is `true`) → publish
///    proceeds with a loud warning logged to the reporter. Use sparingly
///    (incident response, bootstrap runs). The skip decision is
///    operator-visible in stderr; it does *not* synthesize a passing
///    rehearsal receipt, so the audit trail still shows "no rehearsal
///    ran."
///
/// 3. **No rehearsal receipt** (`rehearsal.json` is missing) → refuse.
///    The operator needs an accepted rehearsal receipt first.
///
/// 4. **Stale receipt** (receipt exists but `plan_id` mismatches the
///    current workspace's plan) → refuse. A workspace change between
///    rehearse and publish invalidates the rehearsal.
///
/// 5. **Failing receipt** (`passed: false`) → refuse.
///
/// 6. **Fresh passing receipt for current plan** → publish proceeds.
pub(crate) fn enforce_gate(
    ws: &PlannedWorkspace,
    opts: &RuntimeOptions,
    state_dir: &Path,
    reporter: &mut dyn Reporter,
) -> Result<()> {
    let Some(rehearsal_name) = opts.rehearsal_registry.as_deref() else {
        return Ok(());
    };

    if opts.rehearsal_skip {
        reporter.warn(&format!(
            "--skip-rehearsal was set; publish is proceeding without a rehearsal against '{rehearsal_name}'. \
             This is an operator-authorized bypass; auditors reading events.jsonl will see no RehearsalComplete event for this plan_id."
        ));
        return Ok(());
    }

    let receipt = crate::state::rehearsal::load_rehearsal(state_dir)
        .context("failed to read rehearsal receipt while enforcing hard gate")?;

    let rehearsal_path = crate::state::rehearsal::rehearsal_path(state_dir);

    let receipt = match receipt {
        Some(r) => r,
        None => bail!(
            "rehearsal is required (rehearsal registry '{rehearsal_name}' is configured) but no rehearsal receipt was found at {}. \
             Run `shipper rehearse --registries {rehearsal_name} --rehearsal-registry {rehearsal_name}` first, \
             or pass --skip-rehearsal to override (not recommended).",
            rehearsal_path.display()
        ),
    };

    if receipt.plan_id != ws.plan.plan_id {
        bail!(
            "rehearsal receipt is stale: rehearsal ran for plan_id {} but the current plan_id is {}. \
             The workspace changed between rehearse and publish; re-run `shipper rehearse --registries {rehearsal_name} --rehearsal-registry {rehearsal_name}` against the current plan.",
            receipt.plan_id,
            ws.plan.plan_id
        );
    }

    if !receipt.passed {
        bail!(
            "rehearsal against '{}' did NOT pass for plan_id {}: {}. \
             Fix the cause and re-run `shipper rehearse --registries {rehearsal_name} --rehearsal-registry {rehearsal_name}` before publishing.",
            receipt.registry,
            receipt.plan_id,
            receipt.summary
        );
    }

    reporter.info(&format!(
        "rehearsal gate: passing receipt found ({} packages against '{}', plan_id {})",
        receipt.packages_published, receipt.registry, receipt.plan_id
    ));
    Ok(())
}
