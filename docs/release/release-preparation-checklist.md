# Release preparation checklist

Copy this file to `docs/release/<version>-preparation.md` for each release candidate. Fill every identity and evidence field with an exact value or mark it `BLOCKED`, `NOT RUN`, `NOT APPLICABLE`, or `SUPERSEDED` with a reason. Do not use `pending`, an in-progress job, an earlier head, or an uncited local recollection as proof.

This checklist coordinates two authorities:

- `EffortlessMetrics/shipper-swarm` prepares and freezes the source candidate.
- `EffortlessMetrics/shipper` rehearses, tags, publishes, and retains release-authority evidence.

Command blocks use a POSIX shell for compactness. On Windows, run the equivalent PowerShell commands and record the same identities and results.

## Candidate identity

| Field | Value |
| --- | --- |
| Release version | `TBD` |
| Candidate owner | `TBD` |
| Preparation started | `TBD` |
| Swarm candidate PR or freeze record | `TBD` |
| Swarm candidate SHA | `TBD` |
| Swarm candidate tree | `TBD` |
| Required swarm routed-Rust run | `TBD` |
| Swarm full/equivalent candidate proof | `TBD` |
| Promotion PR in `EffortlessMetrics/shipper` | `TBD` |
| Promotion merge SHA | `TBD` |
| Promotion merge tree | `TBD` |
| Release-authority approved SHA | `TBD` |
| Release-authority approved tree | `TBD` |
| Approval record reference | `TBD` |
| Approval record identifier | `TBD` |
| Approved registry posture | `crates-io` |
| Approved auth posture | `TBD` |
| Reviewed release notes | `RELEASE_NOTES_v<version>.md` |
| Release rehearsal run | `TBD` |
| Release workflow ref | `TBD` |
| Release workflow definition SHA | `TBD` |
| Binary matrix run | `TBD` |
| Interruption/resume proof | `TBD` |
| Tag | `TBD` |
| Tag SHA | `TBD` |
| Publish workflow run | `TBD` |
| Final `.shipper` artifact | `TBD` |
| GitHub Release | `TBD` |
| Post-release swarm backfill SHA | `TBD` |

## Evidence status vocabulary

Use only:

- `PASS` — completed successfully on the recorded exact identity.
- `BLOCKED` — cannot proceed; record the blocker and owner.
- `NOT RUN` — not yet executed and therefore unavailable as evidence.
- `NOT APPLICABLE` — deliberately outside this release; record why.
- `SUPERSEDED` — once valid for an earlier candidate, but not the current identity.

## A. Scope and freeze prerequisites

- [ ] The intended release version is selected deliberately and matches the public compatibility change.
- [ ] The workspace version is identical across the publishable train.
- [ ] The target release section exists in `CHANGELOG.md` and does not remain only under `[Unreleased]`.
- [ ] Every open swarm PR is merged, closed, or listed below as an explicit release exception.
- [ ] Every blocking issue is closed or listed below with a bounded release disposition.
- [ ] `shipper-swarm/main` is green after the last merged change.
- [ ] No normal swarm development PR will merge after the candidate SHA is frozen.
- [ ] The release workflow remains inert in `shipper-swarm`; no publish, tag, signing, or release credential has moved into swarm.

### Explicit exceptions

| Item | Disposition | Why release may proceed | Owner |
| --- | --- | --- | --- |
| `None` |  |  |  |

## B. Changie and changelog preparation

Changie is a maintainer-local authoring and rendering tool. It is not a GitHub Actions merge gate.

- [ ] `changie --version` reports exactly `v1.25.1`.
- [ ] `cargo changelog-roundtrip` passes before any release batch is written.
- [ ] Every `.changes/unreleased/*.yaml` fragment has a deliberate kind, audience, importance, and concise body.
- [ ] Fragment PR references are populated where known.
- [ ] Headline, detailed, and maintenance dispositions were reviewed editorially; the generated order is not accepted blindly.

### Pre-Changie boundary release

For `0.5.0` only:

- [ ] `.changes/0.5.0.md` remains the opaque, verbatim owner of the 0.5.0-through-0.1.0 history.
- [ ] `0.5.0` was **not** batched or regenerated with Changie.
- [ ] `.changes/header.tpl.md` owns the title and `[Unreleased]` boundary exactly once.

### Future releases

For releases after `0.5.0`:

```bash
VERSION=<next-version>
cargo changelog-roundtrip
changie batch "$VERSION"
# Review and edit .changes/$VERSION.md deliberately.
changie merge
cargo changelog-roundtrip
```

- [ ] `changie batch <version>` was run once for this release.
- [ ] `.changes/<version>.md` was reviewed and curated.
- [ ] `changie merge` produced the intended tracked changelog.
- [ ] `cargo changelog-roundtrip` passes after the writing merge.
- [ ] No release section or historical text was dropped, reordered, or silently rewritten.
- [ ] Remaining unreleased fragments are intentional next-release work, or the unreleased ledger is empty.

Synthetic configuration proof, which does not select a real release version:

```bash
changie batch 9999.0.0-baseline-proof --dry-run --allow-no-changes=true
```

- [ ] The synthetic batch dry-run succeeds with the retained baseline present.

## C. Swarm candidate proof

Run from a clean checkout of the proposed swarm candidate:

```bash
set -euo pipefail
SWARM_SHA=<recorded-candidate-sha>
SWARM_TREE=<recorded-candidate-tree>

test -z "$(git status --short)"
test "$(git rev-parse HEAD)" = "$SWARM_SHA"
test "$(git rev-parse HEAD^{tree})" = "$SWARM_TREE"
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

- [ ] Worktree is clean before and after the gate.
- [ ] `cargo fmt` passes.
- [ ] Full-target, all-feature Clippy passes with warnings denied.
- [ ] Workspace nextest passes.
- [ ] Documentation tests pass.
- [ ] All xtask targets and fixture tests pass.
- [ ] Package-surface and policy checks pass.
- [ ] Every publishable crate packages and verifies from its own `.crate` archive in an isolated Cargo target/temporary registry.
- [ ] Advisory documentation contracts have no unowned blocking finding.
- [ ] Changie round-trip passes locally with the pinned binary.
- [ ] `git diff --check` passes.
- [ ] The required `Shipper Rust Small Result` passes on the exact candidate SHA.
- [ ] No substantive automated-review thread remains unresolved.

### Bounded crates.io publish dry-run

A raw `cargo publish --dry-run --workspace` is **not** the all-crate candidate gate. Cargo verifies each package against registry-resolvable dependencies; before the release train starts, dependent 0.5.0 crates correctly stop because their 0.5.0 workspace dependencies are not in crates.io yet. Use the isolated workspace package gate above for the complete 13-crate archive proof.

The five dependency-free crates can still exercise Cargo's publish checks directly:

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
and belong to the dependent train.

- [ ] All five dependency-free publish dry-runs pass.
- [ ] The generated Shipper plan contains exactly 13 publishable crates in topological dependency order.
- [ ] No dependent-crate dry-run failure caused only by an unpublished workspace dependency is misclassified as a package defect.
- [ ] Full dependent-train behavior is covered by the isolated archive gate, release rehearsal, and the actual resumable publish train.

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

On Windows, use `target/install-smoke/bin/shipper.exe`.

- [ ] Facade install succeeds from the candidate tree.
- [ ] Version and primary command help smokes succeed.

### Compatibility and retained proof

- [ ] 0.4 state, event, receipt, and encrypted fixtures still load and resume safely.
- [ ] New artifacts rebuild every field claimed by the current event vocabulary.
- [ ] Sequential/parallel scheduler conformance evidence is current.
- [ ] Registry trust, redirect, resolved-address, and credential-authority evidence is current.
- [ ] Webhook/config diagnostic non-disclosure evidence is current.
- [ ] Semver checks against the published baseline cover all publishable crates, or the candidate record documents an exact non-product diff from a previously proved SHA.

Evidence may be reused only when the record names the earlier SHA, the new SHA, and the complete diff boundary, and explains why that diff cannot affect the claim. The exact-head Rust gate is never reused.

## D. Freeze the swarm candidate

```bash
SWARM_SHA=<recorded-frozen-swarm-sha>
SWARM_TREE=<recorded-frozen-swarm-tree>
```

- [ ] `SWARM_SHA` is merged `shipper-swarm/main`, not a PR merge ref or branch-only commit.
- [ ] `SWARM_TREE` is recorded above.
- [ ] The readiness record names this exact SHA and all evidence used for it.
- [ ] The changelog, release notes, migration notes, support tiers, and carry-over are final for this candidate.
- [ ] No later swarm commit is treated as part of the release without reopening this checklist and rerunning the candidate gate.

## E. History-preserving promotion into `shipper`

Run from an `EffortlessMetrics/shipper` checkout. The source repo must already be an ancestor of the frozen swarm candidate. If it is not, stop and complete a source-backfill into swarm first.

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

- [ ] `origin` is `EffortlessMetrics/shipper` and `swarm` is `EffortlessMetrics/shipper-swarm`.
- [ ] `origin/main` is an ancestor of the frozen `swarm/main` candidate.
- [ ] The sync branch starts from current `shipper/main`.
- [ ] The candidate is merged with `--no-ff`.
- [ ] Promotion tree equals the frozen swarm tree before any separately reviewed release-authority-only change.
- [ ] The promotion PR is merged with a **merge commit**, never squash or rebase.
- [ ] Promotion PR number, merge SHA, and tree are recorded above.
- [ ] Normal swarm development remains paused until the release-authority merge commit is backfilled.

Any conflict, content edit, or tree mismatch reopens candidate review. Do not resolve a promotion conflict as an incidental release-operator edit.

## F. Release-authority proof

The release workflow is intentionally inert in `shipper-swarm`. Run rehearsal and binary modes only in `EffortlessMetrics/shipper` after the promotion merge is on `shipper/main`.

Confirm repository and identity:

```bash
test "$(gh repo view --json nameWithOwner -q .nameWithOwner)" = "EffortlessMetrics/shipper"
git fetch origin --prune --tags
git switch main
git reset --hard origin/main
RELEASE_SHA=$(git rev-parse origin/main)
RELEASE_TREE=$(git rev-parse origin/main^{tree})
```

- [ ] `RELEASE_SHA` is current merged `shipper/main`.
- [ ] Any release-authority-only change after promotion is separately reviewed and recorded.
- [ ] The full candidate gate in section C passes again on `RELEASE_SHA`.
- [ ] `cargo changelog-roundtrip` passes in the release-authority checkout.
- [ ] Release credentials were not exposed to untrusted code or copied into swarm.

Record an immutable release-workflow ref and its definition SHA in the release
record. If the GitHub CLI accepts the approved commit as a workflow ref, use
`RELEASE_SHA`; otherwise use a protected release-preparation tag and record its
definition SHA before dispatching.

The protected `release` environment in `EffortlessMetrics/shipper` must carry
the approved identity handoff as `SHIPPER_APPROVED_RELEASE_VERSION`,
`SHIPPER_APPROVED_RELEASE_SHA`, `SHIPPER_APPROVED_RELEASE_TREE`,
`SHIPPER_APPROVAL_RECORD_REF`, `SHIPPER_APPROVAL_RECORD_SHA`,
`SHIPPER_APPROVED_REGISTRY`, and `SHIPPER_APPROVED_AUTH_POSTURE`. These are
release metadata, not credentials. The workflow identity gate rejects missing
values and validates tag, version, source SHA, source tree, current main,
13-package graph, changelog date, and reviewed release notes.

Accept the identity-gate result only when the workflow definition and `xtask`
validator came from the recorded immutable `WORKFLOW_REF`/`WORKFLOW_SHA` pair;
the approved release SHA/tree and dispatch values are candidate data until
that trusted validator checks them.

Dispatch non-publishing proof on the exact SHA:

```bash
set -euo pipefail
VERSION=<recorded-approved-version>
RELEASE_SHA=<recorded-approved-sha>
RELEASE_TREE=<recorded-approved-tree>
APPROVAL_RECORD_REF=<recorded-approval-record-ref>
APPROVAL_RECORD_SHA=<recorded-approval-record-sha>
APPROVED_AUTH_POSTURE=<recorded-approved-auth-posture>
: "${VERSION:?}"
: "${RELEASE_SHA:?}"
: "${RELEASE_TREE:?}"
: "${APPROVAL_RECORD_REF:?}"
: "${APPROVAL_RECORD_SHA:?}"
: "${APPROVED_AUTH_POSTURE:?}"
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

- [ ] The identity gate passes on the exact approved source and retains its sanitized record.
- [ ] The reversible release proof gate passes on the same source SHA/tree.
- [ ] Rehearsal passes on `RELEASE_SHA` and its artifacts are retained.
- [ ] Four-target binary matrix passes on `RELEASE_SHA` using matching operating-system runners.
- [ ] Binary builds use `--locked` and the approved tracked source and `Cargo.lock` remain unchanged.
- [ ] Every actual archive is extracted and its executable passes exact-version, primary-help, and offline tiny-workspace plan checks on its matching native host.
- [ ] The offline plan reports the expected package/version/dependencies, registry metadata, and no publication.
- [ ] Each retained `<archive>.smoke.json` binds target/host, actual toolchain, source SHA/tree, approved version, lockfile digest, archive/executable digests, and all three successful checks.
- [ ] `verify-binaries` independently validates every uploaded archive's source SHA/tree, target, retention metadata, checksum, executable payload, and matching successful native receipt; missing or skipped proof fails the gate.
- [ ] Synthetic regression fixtures are not counted as native Shipper qualification; all four actual release-candidate archives have current native evidence.
- [ ] Binary checksums and artifacts are retained and unexpired through publication.
- [ ] Interruption/upload and download/resume evidence is current for the approved SHA or an explicitly unchanged execution tree.
- [ ] Auth evidence identifies Trusted Publishing, fallback configuration/use, and selected source without exposing token material.
- [ ] Rehearsal and binary-only runs cannot publish crates or create a GitHub Release.

## G. Tag authorization

Tagging is authorized only when every preceding blocking item is `PASS` and no no-go condition is present.

```bash
VERSION=<version>
RELEASE_SHA=<approved-shipper-main-sha>

WORKSPACE_VERSION=$(cargo metadata --no-deps --format-version 1 \
  | jq -r '.packages[] | select(.name == "shipper") | .version')

test "$WORKSPACE_VERSION" = "$VERSION"
test "$(git rev-parse origin/main)" = "$RELEASE_SHA"
! git rev-parse "v$VERSION" >/dev/null 2>&1

git tag -a "v$VERSION" "$RELEASE_SHA" -m "shipper $VERSION"
test "$(git rev-list -n 1 "v$VERSION")" = "$RELEASE_SHA"
git push origin "v$VERSION"
```

- [ ] Workspace version equals the intended tag.
- [ ] Approved SHA is still current `shipper/main`.
- [ ] Tag does not already exist.
- [ ] Tag points exactly to `RELEASE_SHA`.
- [ ] The tag push occurs only in `EffortlessMetrics/shipper`.
- [ ] Tag-time publication is gated by the approved identity, reversible proof, and all four successful binaries.
- [ ] The GitHub Release is bound to `RELEASE_NOTES_v<version>.md`; generated notes are not its authority.
- [ ] Each exact `name@<version>` is verified before publication; the public install smoke is recorded in Section I after publication.
- [ ] Publish workflow run ID is recorded above.

## H. Publication train monitoring

Expected publish order is the topological 13-crate train recorded in the runbook and the generated plan. Monitor the retained `.shipper` state rather than inferring progress from Cargo output alone.

After each package:

- [ ] `events.jsonl` contains the durable transition.
- [ ] `state.json` agrees with the event-derived projection.
- [ ] Registry visibility is confirmed before the next dependent package.
- [ ] Receipt and auth evidence remain sanitized.
- [ ] The uploaded `.shipper` artifact is current enough to resume the train.

Stop the train when any of these occurs:

- event/state/receipt drift;
- `StillUnknown` reconciliation;
- missing or malformed release artifact;
- cross-authority credential or redirect violation;
- tag/workspace/SHA mismatch;
- package version or ownership mismatch;
- binary or release artifact mismatch;
- an unexpected repository, branch, or workflow identity;
- a new commit after approval that has not completed the checklist.

Do not blind-retry an ambiguous publish. Reconcile registry truth first. If the workflow is interrupted, use the retained `.shipper` artifact and the release workflow's `mode=resume` path. Resume requires `state.json`, the event/receipt evidence, and the matching `.shipper/release-identity.json`; it rejects a different source/version/tree/registry/auth posture. Record both the source artifact run and the resume run.

Both publication and continuation upload `shipper-state-final` within their own
run. For each new resume dispatch, set `artifact_run_id` to the immediately prior
publish or resume run that retained valid evidence, and record the selected
immutable artifact ID from the job summary. Selection rejects a missing, expired,
ambiguous or wrong-run artifact; it never silently falls back to the original
publication. A successful resume still requires section I's public-release
verification and finalization.

## I. Post-release verification and backfill

- [ ] All 13 crate versions are visible on crates.io.
- [ ] `cargo install shipper --version <version> --locked` succeeds from crates.io.
- [ ] Installed binary reports the expected version and primary help surfaces work.
- [ ] GitHub Release exists with the four expected binary artifacts and checksums.
- [ ] Final `.shipper` state, event log, receipt, plan, preflight, and auth evidence are retained.
- [ ] Release readiness record is updated with tag, run IDs, artifacts, and any bounded carry-over.
- [ ] No credential value appears in logs or committed evidence.

Backfill the release-authority merge and any final release-only commits before normal swarm work resumes:

```bash
# From the shipper checkout
FINAL_SOURCE_SHA=<recorded-final-shipper-main-sha>

git fetch origin --prune --tags
git fetch swarm --prune

test "$(git rev-parse origin/main)" = "$FINAL_SOURCE_SHA"
git merge-base --is-ancestor swarm/main "$FINAL_SOURCE_SHA"
git push swarm "$FINAL_SOURCE_SHA":main
```

- [ ] The ancestry check passes before the fast-forward.
- [ ] `shipper-swarm/main` is fast-forwarded to final `shipper/main` without force, squash, or rebase.
- [ ] `git rev-list --left-right --count origin/main...swarm/main` reflects the expected synchronized shape.
- [ ] Normal swarm development is reopened only after the authority relationship is healthy.

If swarm advanced after the freeze, do not overwrite it. Use the documented source-backfill merge path instead.

## No-go conditions

A release is **not authorized** when any item below is true:

- the exact swarm candidate SHA is unknown or unmerged;
- required proof belongs to a different SHA without a documented evidence-reuse boundary;
- `cargo changelog-roundtrip` fails;
- 0.5.0 history was regenerated or historical prose changed unintentionally;
- unresolved release-blocking issue, review thread, or policy finding exists;
- swarm and release-authority trees differ unexpectedly at promotion;
- release workflow is being invoked from `shipper-swarm`;
- release credentials are missing, over-broadly exposed, or moved into swarm;
- rehearsal, binary, package, interruption/resume, or auth evidence is failed or unavailable;
- tag version, workspace version, approved SHA, and tag SHA do not all agree;
- publication state is ambiguous and registry truth has not reconciled it;
- an approved SHA has been superseded by a later commit.

## Final authorization record

| Decision | Value |
| --- | --- |
| Publication authorized | `NO` |
| Authorized by | `TBD` |
| Authorized at | `TBD` |
| Exact approved SHA | `TBD` |
| Blocking exceptions | `None` |
| Notes |  |
