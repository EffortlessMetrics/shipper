# Shipper change fragments

Shipper uses [Changie](https://changie.dev/) to capture release-note material while implementation context is still fresh. Fragments are authoring inputs; `CHANGELOG.md`, release notes, migration notes, support tiers, and readiness records remain deliberately edited and reviewed release artifacts.

## Local setup

Install **Changie v1.25.1**, then install the repository-owned hook:

```bash
changie --version
cargo precommit install
cargo precommit status
```

Common installation paths:

```bash
# macOS
brew install changie

# Windows
winget install miniscruff.changie
```

These package-manager commands install the current packaged release. The hook checks for v1.25.1 exactly; when Changie advances, update the repository pin and local tool together rather than silently accepting changed rendering behavior. A manual v1.25.1 binary can also be downloaded from Changie's GitHub Releases page and placed on `PATH`.

The hook is local-only. No GitHub Actions workflow installs or runs Changie. It validates the staged Git index so unstaged working-tree edits cannot satisfy or break the check. The installed dispatcher also boots Cargo, `.cargo/config.toml`, and `xtask` from a private checkout of the staged index; an unstaged tooling edit cannot replace the implementation that decides the commit.

Before materializing or executing the staged checkout, both the installed dispatcher and the Rust command reject any Git index entry with symbolic-link mode `120000`. This repository-owned gate deliberately does not follow staged symlinks, because a link can escape the private checkout and expose unstaged files to Cargo or Changie. A future contained-symlink policy must be reviewed separately before symlinks can enter this execution surface.

A previously installed v1 Shipper hook is classified as stale and can be upgraded by running `cargo precommit install` again. A customized or truncated current-version hook is classified as conflicting and is never overwritten automatically.

## Add a fragment

For a user-visible or compatibility-relevant change:

```bash
changie new
```

Choose the changelog kind, primary audience, and release-note importance. The optional PR number may be added after the pull request exists. Stage the resulting file under `.changes/unreleased/` with the implementation.

Use the editorial fields deliberately:

- **Headline** — a major operator, compatibility, recovery, security, or usability reason to care about the release.
- **Detailed** — meaningful release detail that belongs in the changelog but not the release-note opening.
- **Maintenance** — repository, test, CI, or internal work worth retaining below user-facing changes.

The pre-commit gate requires a branch-local fragment when staged changes touch product Rust, public manifests and READMEs, user-facing guides, release workflow behavior, templates, or the toolchain floor. Test-only, snapshot-only, policy, ordinary CI, agent-control, and internal status-document changes are exempt by path.

For an unusual behavior-preserving change that matches a release-note path but genuinely needs no fragment, keep the rest of the gate active and provide a substantive reason of at least 12 characters:

```bash
SHIPPER_PRECOMMIT_CHANGELOG_EXEMPT="test-only inline module" git commit
```

PowerShell:

```powershell
$env:SHIPPER_PRECOMMIT_CHANGELOG_EXEMPT = "test-only inline module"
git commit
Remove-Item Env:SHIPPER_PRECOMMIT_CHANGELOG_EXEMPT
```

The reason is retained in `target/hooks/pre-commit.json` and should also be stated in the PR. `git commit --no-verify` remains Git's emergency bypass, but it skips the entire local gate rather than only the fragment requirement.

## Validate manually

```bash
cargo precommit
```

When Changie surfaces are relevant, the command requires v1.25.1 and runs a dry-run batch against the staged snapshot. A dry-run may allow an empty unreleased ledger only when no release-note-relevant path and no branch-local fragment are present, as after a deliberate release batch. A staged product change without a fragment still fails before Changie validation runs.

The command writes an advisory receipt to:

```text
target/hooks/pre-commit.json
```

Use `SHIPPER_PRECOMMIT_BASE=<ref>` when the branch should be compared with something other than `origin/main`.

## Retained pre-Changie history

The tracked changelog through **0.5.0** predates Changie. Its complete 0.5.0-through-0.1.0 body is retained verbatim in:

```text
.changes/0.5.0.md
```

`.changes/header.tpl.md` owns the changelog title and `[Unreleased]` boundary. The historical baseline is intentionally opaque: do not split it, rewrite old prose as fragments, or run `changie batch 0.5.0`.

Prove the retained files reproduce the tracked changelog before and after any Changie configuration, historical baseline, or changelog edit:

```bash
cargo changelog-roundtrip
```

The command requires exactly Changie v1.25.1, runs `changie merge --dry-run`, permits only a zero-versus-one final-newline difference, and reports the first material mismatch. Pure xtask fixture tests also protect historical section ownership without installing Changie in CI.

A synthetic render proof that does not select a real release version is:

```bash
changie batch 9999.0.0-baseline-proof \
  --dry-run \
  --allow-no-changes=true
```

## Prepare a later release

For releases after 0.5.0:

```bash
VERSION=<next-version>
cargo changelog-roundtrip
changie batch "$VERSION"
# Deliberately curate .changes/$VERSION.md.
changie merge
cargo changelog-roundtrip
```

The batch output is an editorial starting point. Release notes, migration notes, support-tier claims, and readiness evidence still receive focused review. Changie does not select the release version, authorize a tag, or publish crates.

Use the [release operator runbook](../docs/release-runbook.md) and a copied [release preparation checklist](../docs/release/release-preparation-checklist.md) to record the exact candidate, promotion, rehearsal, tag, and publication evidence.
