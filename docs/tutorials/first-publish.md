# Tutorial: Prepare a first workspace publish

This tutorial prepares a two-crate workspace for its first Shipper release.
The default path stops at safe mock or alternate-registry rehearsal. A
separately fenced final step shows the live crates.io command only for an
authorized release, because a registry upload cannot be undone.

## What you'll learn

- Install the supported Shipper facade.
- Diagnose local blockers before planning.
- Read the dependency-ordered plan and its `plan_id`.
- Interpret the `PROVEN` / `NOT PROVEN` / `FAILED` preflight result.
- Rehearse without touching crates.io.
- Recognize the fence before a real publish.
- Inspect retained evidence with actual public commands.

## What you'll need

- Rust 1.95 or newer.
- A clean workspace with two publishable crates, one depending on the other.
- About 15 minutes.

You need a crates.io account and publish credential only if a maintainer has
separately authorized the optional live step. Do not create throwaway crates on
crates.io for rehearsal; even yanked uploads remain part of registry history.

## 1. Install

The supported install package is the `shipper` facade. The unversioned command
resolves the version currently exposed by the public registry:

```bash
cargo install shipper --locked
shipper --version
```

The retained public-install evidence for this source line covers 0.4.0. A
source checkout or candidate README does not prove that a later version is
public.

## 2. Create a config

```bash
cd /path/to/your/workspace
shipper config init --output .shipper.toml
```

Review the generated file. Shipper chooses versions neither here nor during
publish; versions and changelog entries must already be intentional.

## 3. Ask Doctor for local blockers

```bash
shipper doctor
```

Fix local tool, auth, workspace, and state-directory blockers before treating
the plan or preflight result as release evidence.

## 4. Plan the release

```bash
shipper plan
```

Confirm that the publishable crates, skips, and dependency-first order match
your intent. The deterministic `plan_id` is the identity Shipper later uses to
refuse recovery against a different workspace plan.

## 5. Run preflight

```bash
shipper preflight
```

Preflight packages the workspace, checks registry and version posture, and
applies the configured policy without publishing. Treat its finishability as a
three-state decision:

- `PROVEN`: every required check was affirmatively proved.
- `NOT PROVEN`: no definitive failure, but at least one proof is missing.
- `FAILED`: a blocker was found; read the reported reason.

`NOT PROVEN` is not permission to publish. Review the named gap; do not convert
unknown evidence into success.

## 6. Rehearse safely

For this workspace's packaged artifacts, configure a non-live registry and run
`shipper rehearse --registries <name> --rehearsal-registry <name>` as described in
[the alternate-registry guide](../how-to/rehearse-against-an-alt-registry.md).
That is the public, non-destructive rehearsal path.

Maintainers with Actions write permission on `EffortlessMetrics/shipper` can
also exercise the release authority's cross-job fake-Cargo/mock-registry
recovery fixture:

```bash
gh workflow run live-runner-interruption-rehearsal.yml \
  --repo EffortlessMetrics/shipper \
  --ref main
```

Neither rehearsal proves crates.io publication.

## 7. Publish only after release authorization

> **Destructive live-registry fence:** the next command can create permanent
> crates.io history. Run it only for final versions from an approved, clean
> release commit after credentials, ownership, rehearsal evidence, and the
> release decision are settled. A normal documentation or pull-request check
> must stop before this step.

```bash
shipper publish
```

Shipper publishes missing versions in dependency order, verifies registry
visibility before advancing, reconciles ambiguous Cargo outcomes against
registry truth, and persists evidence after each package. A `StillUnknown`
outcome stops without a blind retry.

If the run stops, do not immediately rerun it. Follow the
[recovery tutorial](recover-from-interruption.md) and require retained evidence
to agree before resume.

## 8. Inspect what happened

```bash
shipper inspect-events
shipper inspect-receipt
```

The durable contract is:

```text
events.jsonl         = append-only authoritative truth
state.json          = resumable projection
receipt.json        = derived end-of-run summary
reconciliation.json = registry-truth evidence for ambiguous outcomes
```

`reconciliation.json` is conditional; it appears when an ambiguous Cargo
outcome is reconciled. See [INVARIANTS.md](../INVARIANTS.md) for the authority
rules.

## 9. What's next

- For CI, follow [Run a Shipper release in GitHub Actions](../how-to/run-in-github-actions.md).
- For interruption triage, use [Inspect a stalled run](../how-to/inspect-a-stalled-run.md).
- For the exact command surface, run `shipper --help` and each command's
  `--help` output.
