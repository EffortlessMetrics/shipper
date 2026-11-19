# How-to: Remediate a Compromised Release

You shipped a release. Later you discover that one or more crates in that
release are compromised: a CVE, a leaked token in a debug impl, a broken build
artifact. You need to contain new resolves, publish clean successors, and keep
an audit trail that explains what happened.

This guide uses Shipper's current proof-backed remediation boundary:

- `shipper remediate --dry-run` writes a reviewable
  `.shipper/remediation-plan.json` artifact.
- `shipper remediate --execute-plan` executes only the reviewed containment
  yanks recorded in that artifact.
- Fix-forward remains an operator step: Shipper suggests successor versions,
  but does not edit manifests or publish replacements for you.
- Live crates.io remediation is an operator action. PR CI proves the guarded
  execution path with fake Cargo and mock surfaces, not live registry yanks.

> **Containment is not undo.** `cargo yank` prevents new resolves for a
> version; it does not invalidate existing `Cargo.lock` pins or remove already
> downloaded bytes. Existing consumers still need a clean successor and a
> `cargo update`.

## Prerequisites

- A `receipt.json` from the compromised release, usually
  `.shipper/receipt.json`.
- The crate name and version where the compromise starts.
- An incident reason from a CVE, audit report, security ticket, or operator
  note.
- Registry credentials only if you choose to execute yanks. The dry-run plan
  only needs the receipt and workspace context.

## Step 1 - Generate a Remediation Plan

Start with a dry run. This is the safest default because it creates the
evidence artifact before any irreversible registry action:

```bash
shipper remediate --dry-run \
  --from-receipt .shipper/receipt.json \
  --crate my-lib \
  --target-version 0.3.0 \
  --reason "CVE-2026-0001: token leak via Debug impl"
```

This writes:

```text
.shipper/remediation-plan.json
```

The plan records:

- source receipt path
- target crate and version
- affected packages
- reverse-topological yank order
- fix-forward suggestions
- risk notes
- command sequence for reviewed containment

No yanks, manifest edits, or publishes happen during `--dry-run`. The durable
artifact also redacts the operator-supplied reason text and uses a placeholder
in generated commands, so incident details do not leak into uploaded evidence
unless you deliberately record them elsewhere.

## Step 2 - Review the Plan

Inspect `.shipper/remediation-plan.json` before execution. At minimum, confirm:

- `source_receipt` points at the compromised release receipt.
- `target.crate` and `target.version` match the bad crate version.
- `affected_packages` covers the expected downstream crates.
- `yank_order` is dependents-first.
- `fix_forward_suggestions` is publish-directional.
- `risk_notes` match the current support boundary.
- `command_sequence` contains only yanks you intend to run.

Keep the plan with the incident record. It is the evidence object that explains
which containment action was reviewed.

## Step 3 - Execute Containment, If Approved

After review, execute the recorded containment yanks:

```bash
shipper remediate --execute-plan .shipper/remediation-plan.json
```

This command consumes the reviewed artifact and runs only the recorded yank
steps. It does not edit manifests, publish fix-forward successors, or invent new
commands. It halts on the first failed yank and records `PackageYanked` event
evidence.

An unusable event-log destination stops the command before Cargo starts. If
event persistence fails after Cargo has run, the command stops the remaining
plan and returns a nonzero incomplete-evidence error even with `--quiet`.
Preserve the available files and reconcile the observed Cargo result with the
registry before taking another containment action. A local write failure does
not establish that the registry effect failed.
Execution accepts `--format text`; `--format json` is rejected before any effect.
Use JSON with `remediate --dry-run` to inspect the planning artifact.

Select the same single registry recorded in the reviewed artifact, for example
`shipper --registry private remediate --execute-plan .shipper/remediation-plan.json`.
The legacy `[registry]` configuration also supplies this target; an explicit
`--registry` overrides it. A mismatch is refused before any yank or evidence
write. `yank` and `remediate` reject `--registries` and `--all-registries`.
Cargo receives the selected registry explicitly, including `crates-io` when
that is the target, regardless of Cargo's configured default.

When using `--manifest-path` from another directory, Cargo runs in the selected
workspace and relative state paths resolve there. An absolute `--state-dir`
is used unchanged. Give `--plan` and `--execute-plan` a path accessible from the
caller's directory; guarded remediation still accepts only the reviewed file
in the effective state directory.

The planning commands `plan-yank` and `fix-forward` also use that state
directory for their default receipt. An explicit `--from-receipt` path is
resolved from the caller's directory unless absolute.

For a one-off operator action you can still use the lower-level primitive:

```bash
shipper yank \
  --crate my-lib \
  --version 0.3.0 \
  --reason "CVE-2026-0001: token leak via Debug impl" \
  --mark-compromised
```

Use this direct path only when you intentionally want to combine live yank
execution with receipt annotation for a known crate version. Prefer the
dry-run plan when the compromise may affect multiple workspace crates.

`--mark-compromised` checks that the receipt belongs to the selected registry and
contains the package entry before the yank, then rechecks and amends the receipt
after recording the event.
If that amendment fails, the command returns nonzero while retaining the
recorded yank event. Repair the receipt from retained evidence rather than
repeating the yank to retry a local annotation.

## Step 4 - Plan the Fix-Forward

Containment prevents new resolves from choosing bad versions. It does not give
existing users a clean upgrade path. Use `fix-forward` to plan the successor
release:

```bash
shipper fix-forward --from-receipt .shipper/receipt.json
```

This reads compromised receipt entries and prints a supersession plan:

```text
# fix-forward plan - registry=crates-io, plan_id=abc123
# 1 package(s) marked compromised
# Steps:
#   1. For each crate below, bump the version in its Cargo.toml to the
#      suggested successor (or your preferred bump).
#   2. Commit the bumps; they're part of the fix-forward audit trail.
#   3. Run `shipper publish` to ship the successors in topo order.
#
  1. my-lib: 0.3.0 -> 0.3.0-next
```

`fix-forward` is planning only. It does not edit `Cargo.toml` and does not
invoke `shipper publish`.

The operator steps are:

1. Bump the compromised crate to a clean successor version.
2. Bump any workspace crate that should travel with it.
3. Commit the version changes as part of the incident trail.
4. Run the normal Shipper release path.

## Step 5 - Publish the Clean Successors

Use the standard release workflow:

```bash
shipper plan
shipper preflight
shipper publish
```

Shipper's publish engine handles topological order, retry/backoff, ambiguous
outcome reconciliation, state, events, and receipts. See the
[release runbook](../release-runbook.md) for the production-release checklist.

The new receipt records the successor versions. Downstream users still need to
upgrade their lockfiles to the clean chain.

## Optional: Generate a Yank Plan Without `remediate`

If you already marked receipt entries compromised with `shipper yank
--mark-compromised`, or if you want a text-only containment plan, use
`plan-yank`:

```bash
shipper plan-yank \
  --from-receipt .shipper/receipt.json \
  --compromised-only
```

Output is a copy-pasteable list of `shipper yank` commands:

```text
# yank plan (reverse topological) - registry=crates-io, plan_id=abc123, filter=compromised_only
# 1 entries
  1. shipper yank --crate my-lib --version 0.3.0 --reason <REASON>
```

For multi-crate compromises, yank order is dependents-first: the opposite of
publish order.

## Worked Example

Scenario: you released `core@0.3.0` and `app@0.3.0` as a linked workspace.
Later you discover `core@0.3.0` leaks a credential. `app@0.3.0` is not bad on
its own, but it depends on the bad `core`, so it may need containment and a
clean successor.

```bash
# 1. Generate the reviewed plan.
shipper remediate --dry-run \
  --from-receipt .shipper/receipt.json \
  --crate core \
  --target-version 0.3.0 \
  --reason "CVE-2026-0001"

# 2. Review .shipper/remediation-plan.json.

# 3. Execute containment if approved.
shipper remediate --execute-plan .shipper/remediation-plan.json

# 4. Plan and perform the fix-forward.
shipper fix-forward --from-receipt .shipper/receipt.json
# Bump Cargo.toml: core -> 0.3.1, app -> 0.3.1.
# Commit the version changes, then:
shipper plan
shipper preflight
shipper publish
```

End state:

- `core@0.3.0` is contained if its reviewed yank step succeeded.
- `app@0.3.0` is contained if the plan included and executed it.
- `core@0.3.1` is live and clean after the fix-forward publish.
- `app@0.3.1` is live and clean after the fix-forward publish.
- Existing users run `cargo update -p core -p app` to move to the clean chain.

## What Remediation Does Not Cover

- **Lockfile invalidation.** Yanking does not force downstream lockfiles to
  update.
- **Automatic version bumping.** Shipper suggests fix-forward versions; you edit
  manifests and commit the changes.
- **Automatic successor publishing.** Shipper does not publish clean successors
  from a remediation plan.
- **Consumer notifications.** Publish RustSec advisories, release notes, or
  other disclosure material through your normal security process.
- **Live-registry proof in PR CI.** The guarded execution tests use fake Cargo
  and mock surfaces. Live crates.io yanks remain an explicit operator action.

## See Also

- [Inspect state, events, and receipts](inspect-state-and-receipts.md) -
  understand the audit trail that remediation commands read and write.
- [Release runbook](../release-runbook.md) - the standard publish procedure for
  Step 5.
- [Receipt-driven remediation spec](../specs/SHIPPER-SPEC-0008-receipt-driven-remediation.md) -
  support boundary and proof map.
- [Remediate pillar on the roadmap](../../ROADMAP.md) - where this fits in
  Shipper's release-closure model.
