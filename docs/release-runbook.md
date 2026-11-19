# Shipper release operator runbook

Status: active

This runbook is the ordered procedure for preparing, promoting, rehearsing, publishing, and closing a Shipper release. Use it with a copied [release preparation checklist](release/release-preparation-checklist.md); the checklist is the durable evidence record, while this document explains the procedure and stop decisions.

Command blocks use a POSIX shell for compactness. On Windows, use equivalent PowerShell commands and record the same identities and results.

## Non-negotiable authority boundary

| Repository | Authority | Normal merge method |
| --- | --- | --- |
| `EffortlessMetrics/shipper-swarm` | development, Changie fragments, changelog preparation, candidate proof, source freeze | squash-merged PRs |
| `EffortlessMetrics/shipper` | release rehearsal, tags, crates.io publication, GitHub Release, signing, and credentials | merge commits |

The shared `.github/workflows/release.yml` file is intentionally inert in `shipper-swarm`. Jobs that can rehearse, publish, resume a train, build release artifacts, or create a GitHub Release are guarded by:

```text
github.repository == 'EffortlessMetrics/shipper'
```

Do not add publication credentials to swarm and do not treat a workflow dispatch in swarm as release proof.

## Release sequence

```text
fragment intake and changelog preparation in shipper-swarm
→ exact merged-main candidate proof
→ freeze swarm SHA and tree
→ non-fast-forward promotion into shipper
→ prove promotion tree identity
→ exact release-authority rehearsal and binary matrix
→ authorize and push tag from shipper
→ monitor/resume the topological publish train
→ verify crates.io and GitHub Release
→ backfill final shipper history into shipper-swarm
```

Any commit after the frozen candidate reopens the candidate. Rerun the proof and update the checklist rather than appending a new SHA to old evidence.

## 1. Open the release record

Copy the template:

```bash
VERSION=<version>
cp docs/release/release-preparation-checklist.md \
  "docs/release/${VERSION}-preparation.md"
```

Fill the release version, owner, and current status. Do not populate a SHA until it is merged and immutable in the relevant repository.

Before release preparation begins:

- drain or explicitly disposition the swarm PR queue;
- identify blocking issues and carry-over;
- confirm the selected version matches the compatibility change;
- pause unrelated release-document edits;
- confirm no tag or publication has already occurred.

For the 0.5 line, [the readiness record](release/0.5.0-readiness.md) retains earlier candidate, promotion, semver, and binary evidence. Those records remain useful historical evidence, but a later swarm commit requires a fresh exact candidate and promotion before publication.

## 2. Prepare the changelog with Changie

Changie is local authoring support, not a CI gate. Fragment creation and dry-render validation occur at commit time; release batching and merge are deliberate maintainer actions.

Install and verify the pinned tool:

```bash
changie --version
# must report v1.25.1
cargo precommit install
cargo precommit status
```

### Historical boundary

`CHANGELOG.md` through 0.5.0 predates Changie. The complete historical body is retained verbatim in `.changes/0.5.0.md`, while `.changes/header.tpl.md` owns the title and `[Unreleased]` boundary.

Always prove the baseline before changing release files:

```bash
cargo changelog-roundtrip
```

Never run `changie batch 0.5.0`. The 0.5.0 release section is the opaque pre-Changie baseline and must not be regenerated or reinterpreted.

A synthetic configuration proof may use a deliberately impossible version without writing files:

```bash
changie batch 9999.0.0-baseline-proof \
  --dry-run \
  --allow-no-changes=true
```

### Releases after 0.5.0

Review every unreleased fragment before batching. Then:

```bash
VERSION=<next-version>
cargo changelog-roundtrip
changie batch "$VERSION"
# Deliberately curate .changes/$VERSION.md.
changie merge
cargo changelog-roundtrip
```

The generated version file is an editorial starting point. Reorder and edit it to communicate the release accurately, but do not rewrite retained historical files. Confirm that remaining unreleased fragments are intentional next-release work.

## 3. Prove the swarm candidate

Run from a clean checkout of `shipper-swarm/main` after every intended release change is merged:

```bash
set -euo pipefail
test -z "$(git status --short)"
git rev-parse HEAD
git rev-parse HEAD^{tree}

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo nextest run --workspace --all-targets --all-features --locked --profile ci
cargo test --workspace --doc --locked
cargo test -p xtask --all-targets --locked

cargo xtask package-surface
cargo xtask check-lint-policy
cargo xtask no-panic check --mode blocking
cargo xtask check-file-policy --mode blocking-allowlist
cargo xtask check-process-policy --mode blocking-allowlist
cargo xtask check-network-policy --mode blocking-allowlist
cargo xtask check-workflow-surfaces --mode blocking-allowlist
cargo xtask check-doc-contracts --mode advisory
cargo xtask policy-report

PACKAGE_TARGET_DIR=$(mktemp -d)
trap 'rm -rf "$PACKAGE_TARGET_DIR"' EXIT
CARGO_TARGET_DIR="$PACKAGE_TARGET_DIR" \
  cargo package --workspace --locked --exclude xtask

cargo changelog-roundtrip
git diff --check
```

The isolated package gate is the complete 13-crate archive proof. It makes Cargo verify each generated `.crate` archive with intra-workspace path dependencies resolved through the archives produced by the same command, while avoiding stale temporary-registry state from a prior commit.

A raw `cargo publish --dry-run --workspace` is not the pre-release all-crate gate. Cargo's publish verification resolves registry dependencies; dependent 0.5.0 crates correctly stop before publication because their 0.5.0 workspace dependencies are not yet present in crates.io.

Exercise Cargo's publish checks directly for the five dependency-free crates:

```bash
for crate in \
  shipper-cargo-failure \
  shipper-duration \
  shipper-encrypt \
  shipper-output-sanitizer \
  shipper-sparse-index
do
  cargo publish --dry-run --locked -p "$crate"
done
```

`shipper-webhook` and `shipper-retry` left this set when their configuration
values moved to `shipper-types` (#261); both now publish after `shipper-types`
and are covered by the dependent train.

Do not misclassify a dependent-crate dry-run failure caused solely by an unpublished workspace dependency as a package defect. The dependent train is covered by the isolated package gate, release rehearsal, and the resumable live publish train.

Also require the normalized GitHub branch-protection result on the exact merged candidate:

```text
Shipper Rust Small Result
```

Do not substitute a PR merge ref, an earlier branch head, a queued job, or a route-specific implementation job. Review output must be complete enough to show that no substantive thread remains unresolved.

### Install-facade smoke

```bash
rm -rf target/install-smoke
cargo install --path crates/shipper --locked --root target/install-smoke

target/install-smoke/bin/shipper --version
target/install-smoke/bin/shipper doctor --help
target/install-smoke/bin/shipper plan --help
target/install-smoke/bin/shipper publish --help
target/install-smoke/bin/shipper resume --help
```

Use `shipper.exe` on Windows.

### Compatibility and product evidence

The release record must identify current evidence for:

- loading and resuming 0.4 state, event, receipt, and encrypted artifacts;
- rebuilding current state from events for every claimed field;
- sequential/parallel scheduler conformance;
- interruption and artifact handoff;
- ambiguous-outcome reconciliation;
- registry destination, redirect, resolved-address, and credential-authority policy;
- webhook and authorization non-disclosure;
- package surface and semver compatibility across all 13 publishable crates;
- release binary targets on matching operating systems.

An earlier proof may be reused only when the checklist records both SHAs, the complete diff boundary, and why that diff cannot affect the claim. The exact-head Rust gate and release-authority rehearsal are always rerun.

## 4. Freeze the candidate

After the gate passes on merged swarm main:

```bash
SWARM_SHA=<recorded-frozen-swarm-sha>
SWARM_TREE=<recorded-frozen-swarm-tree>
printf 'swarm_sha=%s\nswarm_tree=%s\n' "$SWARM_SHA" "$SWARM_TREE"
```

Record both values and all workflow run IDs in the release checklist and readiness record. Freeze normal swarm merges. If any later commit is required, explicitly supersede this identity and return to section 3.

The candidate must include:

- final changelog and release notes;
- migration and compatibility notes;
- support-tier claim updates;
- release carry-over;
- the completed local Changie baseline proof;
- exact-SHA test, policy, package, and install evidence.

## 5. Promote the candidate into `shipper`

Run from an `EffortlessMetrics/shipper` checkout. The release-authority main branch must already be an ancestor of the frozen swarm candidate. If it is not, stop and use the source-backfill procedure in [SWARM_OPERATION.md](status/SWARM_OPERATION.md) before creating another candidate.

```bash
git remote get-url origin
git remote add swarm git@github.com:EffortlessMetrics/shipper-swarm.git 2>/dev/null || true
git fetch origin --prune --tags
git fetch swarm --prune

test "$(git rev-parse swarm/main)" = "$SWARM_SHA"
test "$(git rev-parse "$SWARM_SHA^{tree}")" = "$SWARM_TREE"
git merge-base --is-ancestor origin/main "$SWARM_SHA"

git switch -c sync/shipper-swarm-YYYY-MM-DD origin/main
git merge --no-ff "$SWARM_SHA" -m "merge: sync shipper-swarm development"

test "$(git rev-parse HEAD^{tree})" = "$SWARM_TREE"
git diff --check
git push -u origin sync/shipper-swarm-YYYY-MM-DD
```

Open the PR in `EffortlessMetrics/shipper`. Merge with a merge commit. Never squash or rebase a swarm promotion.

The tree-equality check is blocking. A normal promotion merge should preserve the frozen swarm tree exactly. A conflict or content edit is not routine release preparation; return the source-authority change to swarm through a reviewed backfill, rebuild the candidate, and promote again.

Record:

- promotion PR;
- merge SHA;
- merge tree;
- proof that the merge tree equals `SWARM_TREE`.

Normal swarm development remains paused until the resulting `shipper/main` merge commit is backfilled.

## 6. Prove the release authority

After promotion is merged, operate only in `EffortlessMetrics/shipper`:

```bash
test "$(gh repo view --json nameWithOwner -q .nameWithOwner)" = \
  "EffortlessMetrics/shipper"

git fetch origin --prune --tags
git switch main
git reset --hard origin/main

RELEASE_SHA=$(git rev-parse HEAD)
RELEASE_TREE=$(git rev-parse HEAD^{tree})
```

Any release-authority-only change after promotion must be a separate reviewed PR. Update the approved SHA and rerun the full candidate gate after it lands.

Record an immutable release-workflow ref and its definition SHA in the release
record. If the GitHub CLI accepts the approved commit as a workflow ref, use
`RELEASE_SHA`; otherwise use a protected release-preparation tag and record its
definition SHA before dispatching.

Run the section 3 commands again in the release-authority checkout, including `cargo changelog-roundtrip`.

The release workflow consumes the approved identity through the protected
`release` environment. Before any tag or dispatch, populate these non-secret
environment variables from the reviewed GO record:

```text
SHIPPER_APPROVED_RELEASE_VERSION
SHIPPER_APPROVED_RELEASE_SHA
SHIPPER_APPROVED_RELEASE_TREE
SHIPPER_APPROVAL_RECORD_REF
SHIPPER_APPROVAL_RECORD_SHA
SHIPPER_APPROVED_REGISTRY=crates-io
SHIPPER_APPROVED_AUTH_POSTURE=trusted_publishing|fallback_secret
```

The tag-time identity job rejects missing values, a changed `shipper/main`, a
tag/version/SHA/tree mismatch, a changed 13-crate package graph, a missing or
wrong-date `CHANGELOG.md` section, or missing
`RELEASE_NOTES_v<version>.md`. Dispatch inputs may override the environment
values for rehearsal, binary-only, and resume proof, but they must still match
the approved source identity. `target/policy/release-identity.json` is the
sanitized handoff artifact; it contains no credential material.

The release-authority workflow definition and its `xtask` validator are trusted
only when sourced from the recorded immutable `WORKFLOW_REF`/`WORKFLOW_SHA`
pair. The approved release SHA, tree, version, tag, and artifact values are
candidate data until that validator checks them; never run the validator from a
mutable branch or accept release proof whose workflow-definition identity was
not independently recorded and matched.

The release-authority coordination issues consume this contract as follows:

- `EffortlessMetrics/shipper#476` records the approved version, SHA, tree,
  approval-record identifier, registry posture, auth posture, workflow
  definition identity, reviewed release notes, and binary dependency before
  marking rehearsal/GO readiness.
- `EffortlessMetrics/shipper#477` refuses tag/publication/resume unless those
  fields are present and the tag-time identity gate, reversible release gate,
  exact four-platform binary matrix, reviewed-note binding, and exact public
  verification all pass on the same approved source.

Those release-authority issues remain separate from swarm implementation; no
release credential or publication capability belongs in this repository.

The tag-time dependency graph is fail-closed:

```text
release-identity-gate ──> build-binaries ──> verify-binaries ──┐
                         release-proof-gate ───────────────────┼─> publish-crates-io ──> exact public verification ──> create-release
                         msrv-gate + policy-gate ──────────────┘
```

The reversible proof gate runs the full Rust, package, and policy ladder on the
approved source. The binary verification gate downloads each retained artifact
and checks its source SHA/tree, target, retention metadata, checksum, and native
executable smoke receipt before publication. Each matrix job builds with
`--locked`, extracts its actual archive, verifies the approved embedded version
and primary help, and executes an offline plan for a tiny dependency-free Cargo
workspace. The plan must report the expected package and registry metadata and
confirm that no publication occurred. Publication cannot begin if the proof, MSRV/policy checks, any
binary matrix child, or binary verification is failed, skipped, unavailable,
expired, or tied to a different SHA/tree.

### Non-publishing rehearsal

Dispatch against the exact approved SHA:

```bash
set -euo pipefail
: "${VERSION:?set VERSION from the approved release record}"
: "${RELEASE_SHA:?set RELEASE_SHA from the approved release record}"
: "${RELEASE_TREE:?set RELEASE_TREE from the approved release record}"
: "${APPROVAL_RECORD_REF:?set APPROVAL_RECORD_REF from the approved release record}"
: "${APPROVAL_RECORD_SHA:?set APPROVAL_RECORD_SHA from the approved release record}"
: "${APPROVED_AUTH_POSTURE:?set APPROVED_AUTH_POSTURE from the approved release record}"
case "$APPROVED_AUTH_POSTURE" in
  trusted_publishing|fallback_secret) ;;
  *) echo "unsupported approved auth posture" >&2; exit 1 ;;
esac

WORKFLOW_REF=<recorded-immutable-release-workflow-ref>
WORKFLOW_SHA=<recorded-release-workflow-definition-sha>
test "$(git rev-parse "$WORKFLOW_REF^{commit}")" = "$WORKFLOW_SHA"

gh workflow run release.yml \
  --repo EffortlessMetrics/shipper \
  --ref "$WORKFLOW_REF" \
  -f mode=rehearse \
  -f ref="$RELEASE_SHA" \
  -f approved_sha="$RELEASE_SHA" \
  -f approved_tree="$RELEASE_TREE" \
  -f approved_version="$VERSION" \
  -f approval_record_ref="$APPROVAL_RECORD_REF" \
  -f approval_record_sha="$APPROVAL_RECORD_SHA" \
  -f approved_registry=crates-io \
  -f approved_auth_posture="$APPROVED_AUTH_POSTURE"
```

The rehearsal must produce and retain plan, preflight, state, event, receipt, auth, and policy evidence without publishing to crates.io.

### Binary matrix

```bash
gh workflow run release.yml \
  --repo EffortlessMetrics/shipper \
  --ref "$WORKFLOW_REF" \
  -f mode=binaries \
  -f ref="$RELEASE_SHA" \
  -f approved_sha="$RELEASE_SHA" \
  -f approved_tree="$RELEASE_TREE" \
  -f approved_version="$VERSION" \
  -f approval_record_ref="$APPROVAL_RECORD_REF" \
  -f approval_record_sha="$APPROVAL_RECORD_SHA" \
  -f approved_registry=crates-io \
  -f approved_auth_posture="$APPROVED_AUTH_POSTURE"
```

Require four matching-platform artifacts:

- Linux x86_64;
- macOS x86_64;
- macOS arm64;
- Windows x86_64.

Record the workflow runs, artifact names, checksums, source SHA/tree, and expiry
window. Each archive has a `<archive>.smoke.json` receipt binding its exact digest
and extracted executable digest to the approved source/version, unchanged
`Cargo.lock`, native target/host, actual Rust/Cargo toolchain, and successful
version, help, and offline-plan checks. The workflow's `verify-binaries` job
reopens all four archives and requires matching successful receipts; missing,
skipped, wrong-version, wrong-architecture, invalid-payload, or mismatched-digest
evidence fails the gate. Its aggregate receipt is retained for 90 days, and the
per-archive receipts accompany the eventual GitHub Release assets.

The nonpublishing regression harness uses explicit synthetic protocol fixtures
to exercise refusals. Those fixtures do not qualify Shipper on any native target:
all four actual archived Shipper binaries must pass on their matching operating
systems for the exact approved release candidate. Publication remains blocked
until every exact-source build and verification succeeds.
Rehearsal and binary-only modes cannot publish crates or create a GitHub
Release. Do not tag if artifacts will expire before publication or if a target
used the wrong operating-system runner.

### Authentication posture

Review the release auth evidence. It must identify observed OIDC context, token minting, fallback configuration/use, and selected auth source without retaining token values. Trusted Publishing may remain advisory when the fallback-token path is the proved release posture; the checklist must state which path is actually authorized.

## 7. Authorize the tag

Tag only when every blocking checklist item passes and the approved release-authority SHA is still current.

```bash
VERSION=<version>
RELEASE_SHA=<approved-shipper-main-sha>
RELEASE_TREE=<approved-shipper-main-tree>

WORKSPACE_VERSION=$(cargo metadata --no-deps --format-version 1 \
  | jq -r '.packages[] | select(.name == "shipper") | .version')

test "$WORKSPACE_VERSION" = "$VERSION"
test "$(git rev-parse origin/main)" = "$RELEASE_SHA"
! git rev-parse "v$VERSION" >/dev/null 2>&1

git tag -a "v$VERSION" "$RELEASE_SHA" -m "shipper $VERSION"
test "$(git rev-list -n 1 "v$VERSION")" = "$RELEASE_SHA"
git push origin "v$VERSION"
```

The tag push starts the irreversible release workflow only in `EffortlessMetrics/shipper`.

Record the tag, tag SHA, and workflow run immediately. If any identity differs, stop before publication begins.

## 8. Monitor the publish train

The generated Shipper plan is authoritative for package order. The current public surface is:

- `shipper-cargo-failure`
- `shipper-duration`
- `shipper-encrypt`
- `shipper-output-sanitizer`
- `shipper-sparse-index`
- `shipper-types`
- `shipper-registry`
- `shipper-retry`
- `shipper-webhook`
- `shipper-config`
- `shipper-core`
- `shipper-cli`
- `shipper`

Before the first upload, confirm the plan contains exactly the intended versions and a topological order. During the train, treat `events.jsonl` as truth, `state.json` as its projection, and `receipt.json` as a summary.

After each package:

- confirm the durable event exists;
- confirm state projection agrees;
- confirm registry visibility before a dependent package runs;
- confirm the latest `.shipper` artifact can be used for resume;
- confirm logs and artifacts remain sanitized.

### Stop conditions

Stop rather than retry when any of these appears:

- `StillUnknown` reconciliation;
- event/state/receipt drift;
- missing or malformed state artifact;
- tag, version, repository, branch, or SHA mismatch;
- unexpected crate, version, owner, or package count;
- registry trust or credential-authority violation;
- cross-origin authorization forwarding;
- binary or checksum mismatch;
- an approved SHA superseded by a later commit.

Cargo output is a hint. Registry truth is the safety boundary. Never blind-retry an ambiguous publish.

## 9. Resume an interrupted train

Use the release record's approved version and SHA. Identify the original publish
run, or the most recent interrupted resume run, that uploaded the last valid
`shipper-state-final` artifact. Every publish and resume run uses this same name
within its own run. Dispatch a new resume in `EffortlessMetrics/shipper` with that
predecessor's run ID:

```bash
VERSION=<recorded-release-version>
RELEASE_SHA=<recorded-approved-release-sha>
RELEASE_TREE=<recorded-approved-release-tree>
WORKFLOW_REF=<recorded-immutable-release-workflow-ref>
WORKFLOW_SHA=<recorded-release-workflow-definition-sha>

git fetch origin --prune --tags
test "$(git rev-parse origin/main)" = "$RELEASE_SHA"
test "$(git rev-list -n 1 "v$VERSION")" = "$RELEASE_SHA"
test -n "$RELEASE_TREE"
test "$(git rev-parse "$WORKFLOW_REF^{commit}")" = "$WORKFLOW_SHA"

gh workflow run release.yml \
  --repo EffortlessMetrics/shipper \
  --ref "$WORKFLOW_REF" \
  -f mode=resume \
  -f ref="$RELEASE_SHA" \
  -f artifact_run_id=<prior-publish-or-resume-run-id> \
  -f approved_sha="$RELEASE_SHA" \
  -f approved_tree="$RELEASE_TREE" \
  -f approved_version="$VERSION" \
  -f approval_record_ref="$APPROVAL_RECORD_REF" \
  -f approval_record_sha="$APPROVAL_RECORD_SHA" \
  -f approved_registry=crates-io \
  -f approved_auth_posture="$APPROVED_AUTH_POSTURE"
```

The source artifact must contain `state.json`, the event/receipt evidence, and
the matching `.shipper/release-identity.json`. The resume job validates source
SHA/tree, version, tag, registry, approval record, and approved auth posture
before minting a fresh token or doing work. Record both the source artifact run
and the resume run. Before resuming, inspect events and state; after resuming,
verify already-published packages were skipped and no duplicate upload occurred.

Selection is restricted to exactly one nonexpired `shipper-state-final` in the
requested run; the download consumes its immutable artifact ID. Missing, expired,
ambiguous, or mismatched evidence stops the job without falling back to another
run. The job summary records the selected predecessor run and artifact ID. A
`shipper-state-preflight` artifact is not a resume input. If another interruption
occurs, start a new resume dispatch with the just-interrupted resume run's ID;
do not reuse the original publication artifact or rename a download manually.
Use separate dispatches for continuation rather than rerunning the same workflow
run, whose immutable artifact name may already have been uploaded.

Do not create a new tag or start a fresh publish workflow to recover an interrupted train.

## 10. Verify and close the release

After publication:

```bash
cargo install shipper --version "$VERSION" --locked
shipper --version
shipper doctor --help
shipper plan --help
shipper publish --help
shipper resume --help
```

Verify:

- all 13 crate versions are visible on crates.io;
- the facade installs from crates.io;
- the installed version and help surfaces are correct;
- GitHub Release contains the four expected binaries and checksums;
- final `.shipper` plan, preflight, events, state, receipt, auth evidence, and policy reports are retained;
- no credential value appears in logs or committed evidence;
- the readiness record includes the final tag, workflow runs, artifacts, and carry-over.

## 11. Backfill release authority into swarm

Normal swarm development may resume only after the release-authority merge and any final release-only commits are present in `shipper-swarm` history.

When swarm has not advanced:

```bash
# From the shipper checkout
FINAL_SOURCE_SHA=<recorded-final-shipper-main-sha>

git fetch origin --prune --tags
git fetch swarm --prune

test "$(git rev-parse origin/main)" = "$FINAL_SOURCE_SHA"
git merge-base --is-ancestor swarm/main "$FINAL_SOURCE_SHA"
git push swarm "$FINAL_SOURCE_SHA":main
```

Do not force push. If swarm advanced after freeze, use the source-backfill merge procedure in [SWARM_OPERATION.md](status/SWARM_OPERATION.md) instead of overwriting development commits.

Finally verify the ancestry count described in the operation policy and reopen normal swarm merges.

## Release no-go summary

Do not tag or publish when:

- the candidate is not merged swarm main;
- exact-head required proof is incomplete or belongs to another SHA;
- `cargo changelog-roundtrip` fails;
- 0.5.0 was rebatch-generated or history changed unintentionally;
- a substantive review or policy finding remains unresolved;
- promotion tree differs from the frozen swarm tree;
- rehearsal, binaries, interruption/resume, auth, or package evidence is failed or unavailable;
- release workflow is being invoked from swarm;
- credentials have moved into swarm or are exposed to untrusted code;
- workspace version, tag, approved SHA, and tag SHA disagree;
- publication state is ambiguous and registry truth has not reconciled it.

The correct result of an incomplete checklist is **no release**, not an inferred pass.
