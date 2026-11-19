# How to run a Shipper release in GitHub Actions

Goal: an explicitly approved workspace release runs through Shipper and retains
the evidence needed to inspect its result and choose a later recovery operation.

> **Destructive live-registry fence:** the workflow below can publish permanent
> versions. Protect the `release` environment with maintainer approval and use
> it only from the repository that owns release authority. Pull requests and
> ordinary CI should use the fake-Cargo/mock-registry recovery rehearsal, not a
> live publish. `EffortlessMetrics/shipper` remains the release-authority
> repository. Its
> `.github/workflows/release.yml` is the production example; the active
> development repository does not own publish credentials.

## Minimal workflow

This example is for GitHub.com with the Cargo workspace at the checkout root.
Create a protected `release` environment with maintainer approval and its
`CARGO_REGISTRY_TOKEN` secret. Select an exact, approved Shipper version already
available on crates.io whose CLI matches the generator used below. For an
unreleased candidate, install the approved source through the release runbook;
an unreleased version cannot be installed from crates.io.

The final two steps are the output of `shipper ci github-actions`, inserted under
`jobs.publish.steps`. The job installs the tool and performs proof first. Its
preflight uses `--preflight-only`, which writes a session sidecar rather than
creating authoritative publish history that would make the fresh-run step refuse.

```yaml
name: Release

on:
  workflow_dispatch:
    inputs:
      shipper_version:
        description: Exact approved Shipper version available on crates.io
        required: true
        type: string

permissions:
  contents: read

jobs:
  publish:
    runs-on: ubuntu-latest
    environment: release
    timeout-minutes: 180
    env:
      SHIPPER_STATE_DIR: "./.shipper"
    steps:
      - uses: actions/checkout@v6
        with:
          persist-credentials: false

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Install Shipper
        env:
          SHIPPER_VERSION: ${{ inputs.shipper_version }}
        run: cargo install shipper --version "$SHIPPER_VERSION" --locked

      - name: Diagnose release prerequisites
        run: shipper doctor

      - name: Plan
        run: |
          mkdir -p "$SHIPPER_STATE_DIR"
          shipper plan --format json | tee "$SHIPPER_STATE_DIR/plan.txt"

      - name: Upload plan artifact (before anything destructive)
        if: always()
        uses: actions/upload-artifact@v7
        with:
          name: shipper-state-plan-${{ github.run_id }}-${{ github.run_attempt }}
          path: ${{ env.SHIPPER_STATE_DIR }}/
          include-hidden-files: true
          retention-days: 30

      - name: Preflight
        env:
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
        run: shipper preflight --policy safe --preflight-only --state-dir "$SHIPPER_STATE_DIR"

      - name: Upload preflight artifact
        if: always()
        uses: actions/upload-artifact@v7
        with:
          name: shipper-state-preflight-${{ github.run_id }}-${{ github.run_attempt }}
          path: ${{ env.SHIPPER_STATE_DIR }}/
          include-hidden-files: true
          retention-days: 30

      - name: Publish crates from fresh evidence
        shell: bash
        working-directory: ${{ github.workspace }}
        env:
          SHIPPER_STATE_DIR: "./.shipper"
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
        run: |
          if [ -e "$SHIPPER_STATE_DIR" ] || [ -L "$SHIPPER_STATE_DIR" ]; then
            retained=$(find -L "$SHIPPER_STATE_DIR" \( -name state.json -o -name events.jsonl -o -name receipt.json -o -name reconciliation.json \) -print -quit)
            if [ -n "$retained" ]; then
              printf '%s\n' 'Retained release evidence is present. Inspect the selected source artifact and choose an approved recovery operation before publishing.' >&2
              exit 1
            fi
          fi
          shipper publish --quiet --state-dir "$SHIPPER_STATE_DIR"

      - name: Retain Shipper evidence
        if: always()
        uses: actions/upload-artifact@v7
        with:
          name: shipper-state-${{ github.run_id }}-${{ github.run_attempt }}
          path: "./.shipper/"
          include-hidden-files: true
          if-no-files-found: error
```

## Key considerations

### `include-hidden-files: true`

`.shipper/` is a hidden directory. Without this flag, the artifact upload silently skips it. This bit us in rc.1 (issue #89).

### Upload state at every stage

Upload the selected state directory after plan, after preflight, and after
publish or failure. `if: always()` retains available evidence after a command
failure; it cannot upload files after a runner disappears or guarantee an upload
after hard cancellation. Inspect the retained artifact before choosing recovery.
Keep unrelated files and credentials out of this directory: hidden-file inclusion
uploads its hidden contents too.

### Timeout budget

Registry rate limits and visibility delays can change. Budget from a rehearsal
of the actual package graph, retain Shipper's wait/backoff events, and leave
enough workflow time for evidence upload even after a failure. The example uses
180 minutes; that number is an operator budget, not a registry guarantee.

### Token vs Trusted Publishing

The example above uses `CARGO_REGISTRY_TOKEN` — a long-lived personal
access token stored as a repo secret. The retained 0.4.0 Shipper release
evidence proves that fallback path: it recorded Trusted Publishing token
mint failure, a configured fallback secret, and `selected_token_source =
"fallback_secret"` without storing token values.

Trusted Publishing (OIDC) is the target path once crates.io registration and a
release rehearsal prove it for every crate in the workspace. It uses
short-lived tokens scoped to a specific repo, workflow, ref pattern, and GitHub
Actions environment, with no PATs to rotate or leak. Keep the fallback secret
configured until the Trusted Publishing path is proven end to end.

**One-time setup on crates.io** (per crate):

1. Log in to <https://crates.io>, open the crate's **Settings →
   Trusted Publishing** panel.
2. Add a new trusted publisher:
   - Repository: `<owner>/<repo>`
   - Workflow filename: `release.yml` (or whatever yours is called)
   - Environment: `release` (match the `environment:` name in the job
     below — this is the scope guard)
3. Repeat for **every** crate the workspace publishes. Do NOT enable
   OIDC until the list is complete.

> **Why "every crate"**: if only some crates are registered, an exchanged
> token can still fail on an unregistered crate after earlier publishes
> succeeded. Shipper's preflight ownership checks add evidence for existing
> crates, but do not prove all token permissions. New crates have no owner
> record yet, so their first publication still depends on complete setup.
> Complete registration first; rehearse second; tag third.
>
> **Rehearsal checks the mechanism.** `release.yml`'s
> `release-rehearse` job binds to `environment: release` so the OIDC
> context matches the intended production environment. A successful mint
> establishes that the exchange returned a token; it does not establish
> authorization for every crate or successful publication. A mid-train 401
> can indicate missing registration. Check the retained auth evidence and
> per-crate configuration before choosing a recovery action.

**Workflow**:

```yaml
permissions:
  contents: write
  id-token: write           # required to mint the OIDC token

jobs:
  publish:
    runs-on: ubuntu-latest
    environment: release    # must match the crates.io trusted-publisher config
    steps:
      - uses: actions/checkout@v6
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo install shipper --locked

      # Exchange the workflow's OIDC token for a short-lived
      # crates.io publish token. Output: steps.auth.outputs.token.
      - id: auth
        # Allow an approved fallback to be evaluated after a failed mint.
        continue-on-error: true
        uses: rust-lang/crates-io-auth-action@v1

      - name: shipper publish
        shell: bash
        env:
          # Keep the release workflow's selection: minted output first,
          # then the explicitly configured long-lived fallback secret.
          CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token || secrets.CARGO_REGISTRY_TOKEN }}
          SHIPPER_TOKEN_MINTED: ${{ steps.auth.outputs.token != '' }}
        run: |
          set -euo pipefail
          if [[ -z "${CARGO_REGISTRY_TOKEN//[[:space:]]/}" ]]; then
            echo "::error::No usable publish token; publication was not started." >&2
            exit 1
          fi
          if [[ "$SHIPPER_TOKEN_MINTED" == "true" ]]; then
            echo "Publish token source: trusted_publishing"
          else
            echo "::warning::Publish token source: fallback_secret"
          fi
          shipper publish --policy safe
```

The auth action's
[`continue-on-error`](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#jobsjob_idstepscontinue-on-error)
allows a failed mint to reach the explicit fallback selection. A nonempty minted
output wins over the fallback; successful minting also works without a fallback
secret. If the selected token is empty or whitespace-only, the shell exits before
`shipper publish`. A nonempty but unusable minted token does not trigger an
automatic fallback after publication fails. The fixed source labels reveal no
token values. If you add Doctor or preflight steps to this fragment, give each
the same selected-token expression rather than a different credential source.

`shipper doctor` validates the local workflow prerequisites it can see:
`id-token: write`, `environment: release`,
`rust-lang/crates-io-auth-action@v1`, and an explicit
`secrets.CARGO_REGISTRY_TOKEN` fallback. It does not validate crates.io's
per-crate Trusted Publishing registration. Visible OIDC environment variables,
a returned token, per-crate registration, fallback use, and successful publication
are separate observations. Ownership checks for existing crates do not establish
authorization for a new crate's first publication.

The local example fixtures parse this YAML and execute its publish shell with
synthetic tokens and a fake command. They cover minted-token precedence, failed
mint with fallback, minted-only use, and refusal without a usable token. They do
not perform an OIDC exchange, inspect crates.io registration, or publish anything.
The release authority's auth evidence remains owned by
[#105](https://github.com/EffortlessMetrics/shipper/issues/105); this example does
not promote the [Trusted Publishing support tier](../status/SUPPORT_TIERS.md).

When the workflow keeps `secrets.CARGO_REGISTRY_TOKEN` as a fallback,
`shipper doctor` and `shipper preflight` keep that path visible with
advisory warnings. Treat the fallback as the retained proven posture and the
incident-recovery path until newer release evidence proves otherwise. Promote the short-lived token from
`rust-lang/crates-io-auth-action@v1` to the normal release path only after
release evidence proves the token mint path succeeds and fallback use is
explicitly unnecessary for the published crate set.

**Troubleshooting**:

- `id-token: write` missing → GitHub refuses the OIDC exchange → the
  action fails. This example continues to the explicit fallback, or refuses
  publication when no usable token is selected; fix the missing permission.
- Crate not registered as a trusted publisher → `cargo publish` returns
  401 despite a valid-looking token. Check crates.io's Trusted
  Publishing panel for the crate.
- Tag/branch mismatch → token minted for the wrong ref pattern →
  crates.io refuses. The `environment:` name is the tightest scope —
  make sure the workflow's environment matches what you registered.

See the release authority's
[`release.yml`](https://github.com/EffortlessMetrics/shipper/blob/main/.github/workflows/release.yml)
for the production example, and
[#96](https://github.com/EffortlessMetrics/shipper/issues/96) for the migration
history.

### Resume mode

If a release is interrupted, manually trigger the release authority's resume
workflow (a `workflow_dispatch` with `mode: resume` and
`artifact_run_id: <failed run id>`) — or adapt the resume job from the linked
production workflow while preserving its artifact and identity checks.

The generated fresh-publication recipe does not download or cache mutable
execution state. For recovery, explicitly select the source run and the named
`shipper-state-<run_id>-<run_attempt>` artifact, restore it into the matching
source checkout with the matching Shipper binary and registry configuration, and
inspect `shipper status --durable` and the retained events. A missing artifact
is a stopped recovery step, not permission to start a new publication.

`publish` can load existing state, but state existence does not authorize a
retry. A controlled `NotPublished` stop requires its supported, explicitly chosen
resume operation and retry budget. `StillUnknown`, mismatched identity, and
unresolved process-cleanup uncertainty require investigation before another
upload. Do not remove evidence or substitute an unconditional `publish` to bypass
those refusals. The generated fresh-run check leaves retained files untouched
and also checks registry subdirectories. An unreadable directory or symlink loop
stops the check before publication; it does not establish fresh state.

### Exit codes

`shipper publish` and `shipper resume` use a structured exit-code vocabulary so CI can distinguish outcomes:

| Code | Meaning | CI action |
|-----:|---------|-----------|
| 0 | All packages published/skipped | Proceed |
| 1 | General failure (config error, preflight failure, complete publish failure) | Alert / investigate |
| 2 | Finalized partial publish result — some packages published, some failed | Inspect the outcome and retained evidence before deciding whether resume is safe |

Clap/usage errors also exit 2, but print usage and create no `.shipper/`
execution evidence. They are not partial publish results. Do not branch on the
number alone; for a finalized JSON result inspect `execution_result`,
`safe_to_rerun` plus its reason, `next_action`, and evidence references.

Capture the code without hiding it from the job, then upload evidence for an
operator-controlled follow-up:

```yaml
- name: Publish
  id: publish
  shell: bash
  run: |
    set +e
    shipper publish --format json \
      > .shipper/publish.json \
      2> .shipper/publish.stderr
    code=$?
    echo "shipper_exit=$code" >> "$GITHUB_OUTPUT"
    exit "$code"

- name: Upload retained evidence
  if: always()
  uses: actions/upload-artifact@v7
  with:
    name: shipper-state-final
    path: .shipper/
    include-hidden-files: true
```

For a wrapped plan-build or publish-engine failure before a receipt exists,
inspect `.shipper/publish.stderr` for the redacted
`shipper.publish.error.v1` envelope; stdout is intentionally empty in that
case. Parser/usage and config/option-validation errors outside that typed
boundary may instead be prose or usage output. The separate stderr file keeps
either form available; retain both files with the rest of `.shipper/`.

The completed-result JSON envelope carries `execution_result` (`"success"`,
`"partial_failure"`, `"complete_failure"`) for programmatic gating. A later
approved job may restore the exact artifact as `.shipper/` inside the exact
source checkout and run `shipper status --durable` with the matching candidate
binary. It should invoke `shipper resume` only when the evidence-backed rerun
posture allows it.

## Generate and place the step snippet

```bash
shipper ci github-actions > shipper-release-steps.yml
```

This produces a YAML **sequence of steps**, not a complete workflow. Keep the
snippet outside `.github/workflows/` and replace the final two steps of the
complete workflow above with it. Save the assembled workflow as
`.github/workflows/release.yml`. Its deliberate manual trigger, protected release
environment, installation and preflight are part of the wrapper, not generated
publication authority.

For a custom directory, including spaces:

```bash
shipper --state-dir 'release evidence' ci github-actions > shipper-release-steps.yml
```

Set the wrapper's job-level `SHIPPER_STATE_DIR` to `./release evidence` too, so
plan and preflight use the same directory. The generated publish step passes that
path to Shipper and the upload step names the same directory. Paths must be
literal and relative to the checkout/workspace root; absolute paths, `..`, drive
prefixes, backslashes, globs, control characters and workflow expressions are
refused. Spaces, quotes and ordinary Unicode names are supported. The snippet
does not transfer the generator's manifest path or runtime flags: keep the Cargo
workspace at the checkout root and put the intended policy and registry settings
in the reviewed checkout configuration.

The recipe uses GitHub.com's
[`actions/upload-artifact@v7`](https://github.com/actions/upload-artifact), with
hidden-file inclusion and a run/attempt-specific artifact name. Artifact v4 and
later are not supported on GitHub Enterprise Server; this generated recipe does
not claim GHES support. Validate the assembled workflow with the repository's
`actionlint` entry before use. Local fake-Cargo tests establish the command and
file contract, not a hosted artifact transfer or successful live publication.

## See also

- [Publish missing workspace crates](publish-missing-workspace-crates.md) — minimal idempotent publish recipe for CI
- [Tutorial: First publish](../tutorials/first-publish.md)
- [Tutorial: Recover from an interrupted release](../tutorials/recover-from-interruption.md)
- [Release runbook](../release-runbook.md) — operator reference for production releases
