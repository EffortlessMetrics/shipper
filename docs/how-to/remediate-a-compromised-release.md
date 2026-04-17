# How-to: Remediate a compromised release

You shipped a release. Later you discover one or more crates in that
release are compromised — a CVE, a leaked token in a debug impl, a
broken build artifact. You need to:

1. **Contain** the damage (stop new resolves from pulling the bad chain).
2. **Fix-forward** (ship a clean successor so existing consumers have
   something good to upgrade to).
3. **Finalize** (optionally yank the bad versions once the clean chain
   is live).

This guide walks you through all three, using the Remediate commands
landed in [#98](https://github.com/EffortlessMetrics/shipper/issues/98).

> **Containment is not undo.** `cargo yank` prevents NEW resolves for a
> version; it does NOT invalidate existing `Cargo.lock` pins. That's
> why containment alone isn't enough — the operator running `cargo
> build` in an existing checkout will still resolve to the bad version
> unless you ALSO fix-forward and they run `cargo update`.

## Prerequisites

- A `receipt.json` from the compromised release in `<state_dir>`
  (default `.shipper/receipt.json`), produced by the `shipper publish`
  that shipped it.
- Publish credentials for the affected registry (same token / OIDC
  setup you used for the release).
- For a compromised chain spanning multiple crates, you should know
  the compromise scope — which crate names are bad. A CVE
  advisory, an audit report, or an incident ticket should tell you.

## Step 1 — Record the compromise in the receipt

Use `shipper yank --mark-compromised` to yank AND annotate the
receipt in one go. Run this for each compromised crate+version:

```bash
shipper yank \
  --crate my-lib --version 0.3.0 \
  --reason "CVE-2026-0001: token leak via Debug impl" \
  --mark-compromised
```

What this does:

- Invokes `cargo yank --version 0.3.0 my-lib` against the registry.
  The version becomes unresolvable for new dependency resolves.
- Emits a `PackageYanked` event to `events.jsonl`.
- Amends the matching `PackageReceipt` entry in `<state_dir>/receipt.json`:
  - `compromised_at = <now>`
  - `compromised_by = "CVE-2026-0001: token leak via Debug impl"`

The `--mark-compromised` flag is **opt-in** — without it, `shipper yank`
only yanks. We keep it explicit because marking a receipt is an
annotation over an audit document and should be an operator decision.

> **Idempotent.** Running this twice for the same crate+version is
> safe: the second yank is a no-op on crates.io, and the receipt
> amendment just updates `compromised_at` to the latest timestamp.

## Step 2 — Plan the fix-forward

```bash
shipper fix-forward --from-receipt .shipper/receipt.json
```

This reads the receipt, finds every package whose entry was marked
compromised in Step 1, and prints a supersession plan — dependencies
first, dependents last, same direction as the original publish (because
you're *publishing* replacements, not *removing* reachability):

```
# fix-forward plan — registry=crates-io, plan_id=abc123
# 1 package(s) marked compromised
# Steps:
#   1. For each crate below, bump the version in its Cargo.toml to the
#      suggested successor (or your preferred bump).
#   2. Commit the bumps; they're part of the fix-forward audit trail.
#   3. Run `shipper publish` to ship the successors in topo order.
#   4. Once all successors are live, optionally run `shipper plan-yank
#      --from-receipt <path> --compromised-only` to contain the
#      compromised versions.
#
  1. my-lib: 0.3.0 -> 0.3.0-next  # CVE-2026-0001: token leak via Debug impl
```

`fix-forward` is **planning only**. It does not edit Cargo.toml and
does not invoke `shipper publish`. The operator steps are:

1. Bump `my-lib`'s version in its Cargo.toml. `0.3.0-next` is a
   suggestion — use `0.3.1` for a real patch, or whatever your
   SemVer policy dictates.
2. Also bump any workspace crate whose version should travel with
   `my-lib` (typically everything in a single-version workspace).
3. Commit the bumps as a single "fix-forward: <reason>" commit. The
   git history becomes part of the remediation trail.
4. Run `shipper publish` to ship the successors.

Optional `--format json` emits the plan as structured JSON for
programmatic remediation tooling.

## Step 3 — Execute the publish

Same as a normal release train:

```bash
shipper plan      # confirm the bump is what you expect
shipper preflight # git clean, registry reachable, dry-run
shipper publish   # topological train with retry + readiness
```

Shipper's engine handles retry / backoff / ambiguous reconciliation
automatically. See the [release runbook](../release-runbook.md) for
the production-release checklist.

The new receipt records the successors. At this point `my-lib@0.3.0` is
yanked and compromised, `my-lib@0.3.1` is live and clean.

## Step 4 — Finalize: yank the old chain (optional)

If you want belt-and-braces containment, generate a reverse-topological
yank plan from the original compromised receipt and walk it:

```bash
shipper plan-yank \
  --from-receipt .shipper/receipt.json \
  --compromised-only
```

Output is a copy-pasteable list of `shipper yank` commands:

```
# yank plan (reverse topological) — registry=crates-io, plan_id=abc123, filter=compromised_only
# 1 entries
  1. shipper yank --crate my-lib --version 0.3.0 --reason <REASON>  # CVE-2026-0001: token leak via Debug impl
```

For multi-crate compromises, the order is **dependents first** — the
opposite of publish order — so downstream consumers stop being
resolvable BEFORE the bad version they depend on is pulled.

You don't *have* to run Step 4 if you did Step 1 already (Step 1 yanked
each crate as you marked it). This step exists for the case where the
operator marked packages compromised but *didn't* yank them yet (using
a separate `--mark-compromised`-only tool), or where the plan needs
reviewing before yanking.

## Worked example: single-CVE in a multi-crate release

Scenario: released `core@0.3.0`, `app@0.3.0` as a linked workspace.
Later found `core@0.3.0` has a credential-leak bug. `app@0.3.0` is
uncompromised *on its own* but depends on `core@0.3.0`, so it's in
scope for the fix-forward to pick up the clean `core`.

```bash
# 1. Yank + mark just the directly compromised crate
shipper yank \
  --crate core --version 0.3.0 \
  --reason "CVE-2026-0001" \
  --mark-compromised

# 2. Plan the fix-forward
shipper fix-forward
# → tells you to bump core, commit, shipper publish

# 3. Bump Cargo.toml: core -> 0.3.1, app -> 0.3.1 (app picks up clean core)
#    Commit, then:
shipper publish
# → publishes core@0.3.1 and app@0.3.1

# 4. Optionally yank app@0.3.0 too so resolvers can't fall back to it:
shipper yank \
  --crate app --version 0.3.0 \
  --reason "CVE-2026-0001 (transitive via core)" \
  --mark-compromised
```

End state:

- `core@0.3.0` — yanked, marked compromised.
- `app@0.3.0` — yanked, marked compromised (transitive).
- `core@0.3.1` — live, clean.
- `app@0.3.1` — live, clean, depends on `core@0.3.1`.
- Existing `app@0.3.0` users run `cargo update -p core -p app` and
  they're on the clean chain.

## What "remediation" does NOT cover

- **Lockfile invalidation.** Yanking does not force-upgrade downstream
  lockfiles. You cannot un-ship a compromised version; only prevent
  new resolves. Operators running existing checkouts with pinned
  lockfiles continue to resolve to the bad version until they
  `cargo update`.
- **Version bumping.** Shipper doesn't (yet) edit Cargo.toml on your
  behalf. `fix-forward` tells you what to bump; you do the bumping.
- **Consumer notifications.** If your crate has consumers outside your
  control, you still need to announce the CVE (RustSec advisory,
  release notes, security mailing list). Shipper's trail captures the
  technical remediation; it doesn't publish the disclosure.

## See also

- [Inspect state, events, and receipts](inspect-state-and-receipts.md) —
  understanding the audit trail that `--mark-compromised` writes into.
- [Release runbook](../release-runbook.md) — the standard publish
  procedure that Step 3 invokes.
- [Remediate pillar on the roadmap](../../ROADMAP.md) — where this fits
  in Shipper's five-pillar release-closure model.
