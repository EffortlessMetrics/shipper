# How-to: Rehearse a release against an alternate registry

Goal: prove a release will actually publish and resolve cleanly BEFORE
you touch crates.io. This is the Prove pillar's phase-2 check —
`cargo publish --dry-run` validates that Cargo can package the crate,
but it cannot prove that the packaged tarballs will actually install
from a registry index with real workspace-path → registry-path
resolution. That gap is what this guide closes.

## When to use this

- First time publishing a multi-crate workspace (the rc.1 landing mine
  that wasted a tag cycle).
- After any workspace refactor that reshapes dependency edges
  (new intra-workspace deps, path → registry migrations, feature
  reshuffles).
- Before any release you can't afford to retract.

If you've already shipped the workspace once and are just bumping
versions on stable code, rehearsal is overkill. If anything about the
publish shape has changed, rehearse.

## Prerequisites

- An **alternate registry**. Options, rough-ordered by effort:
  - **kellnr** (recommended for CI): self-host a registry sidecar per
    release. [kellnr docs](https://kellnr.io/documentation). Short-
    lived instance — spin up, rehearse, tear down.
  - **Throwaway crates.io account**: works but pollutes the real
    registry with rehearsal versions. Use different crate names if
    you go this route.
  - **Your existing private registry**: if your org already has one
    (cloudsmith / Artifactory / JFrog / Cargo-hosted), a dedicated
    `rehearsal-*` namespace works.

- Registry URL + token configured in Cargo so `cargo publish --registry`
  works.

- A `.shipper.toml` or CLI flag that names the registry for rehearsal.

## Step 1 — Configure the rehearsal registry

### Via `.shipper.toml`

```toml
[[registries]]
name = "rehearsal"
api_base = "https://rehearsal.internal.example.com"
index_base = "https://rehearsal.internal.example.com/api/v1/crates"

[rehearsal]
enabled = true
registry = "rehearsal"
```

`[[registries]]` teaches Shipper about the registry's URLs.
`[rehearsal]` says "when running `shipper rehearse`, target this one."

### Via CLI

```bash
shipper rehearse --rehearsal-registry rehearsal
```

Opts in ad-hoc without editing config. Useful for CI jobs that override
behavior per-run.

## Step 2 — Dry-run in isolation

```bash
shipper rehearse
```

What this does:

1. Validates that the rehearsal registry is configured and differs
   from the live target. (Rehearsing against crates.io would defeat
   the point.)
2. For each crate in the plan (topological order):
   - Runs `cargo publish --registry rehearsal -p <crate>`.
   - Waits for the crate to appear on the rehearsal registry's
     visibility endpoint — same `version_exists` check the live
     path uses, so if the real publish would fail visibility the
     rehearsal fails first.
3. Emits per-step events to `<state_dir>/events.jsonl`:
   - `RehearsalStarted { registry, plan_id, package_count }`
   - `RehearsalPackagePublished { name, version, duration_ms }`
     per success
   - `RehearsalPackageFailed { name, version, class, message }`
     per failure (stops the loop)
   - `RehearsalComplete { passed, registry, plan_id, summary }`
4. Writes a sidecar `<state_dir>/rehearsal.json` — the hard gate
   consults this file later.

Exit status is non-zero on rehearsal failure, so CI lanes that wrap
this command fail automatically.

## Step 3 — The hard gate

Once you have a passing rehearsal, the hard gate blocks live publish
without one:

```bash
shipper publish
```

Decision tree:

1. `rehearsal_registry` not configured → dormant, publish proceeds.
   (Rehearsal is opt-in; workflows that haven't adopted it are
   unaffected.)
2. `--skip-rehearsal` flag set → publish proceeds with a loud warning.
   No fake-passing receipt is synthesized; the audit trail shows the
   bypass honestly. Use sparingly (incident response, bootstrap runs).
3. No `rehearsal.json` → refuse. Run `shipper rehearse` first.
4. `rehearsal.json`'s `plan_id` differs from the current plan →
   refuse (stale). The workspace changed between rehearse and publish.
5. `passed: false` → refuse, citing the rehearsal summary.
6. Fresh passing receipt for the current plan → proceed, info-log
   "rehearsal gate: passing receipt found (N packages against
   'rehearsal', plan_id ...)".

## Example: GitHub Actions with kellnr sidecar

```yaml
name: Release
on:
  push:
    tags: ['v*.*.*']

jobs:
  rehearse:
    runs-on: ubuntu-latest
    services:
      kellnr:
        image: ghcr.io/kellnr/kellnr:latest
        ports:
          - 8000:8000
        env:
          KELLNR_ORIGIN__ADDR: http://localhost:8000
    steps:
      - uses: actions/checkout@v6
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo install shipper-cli --locked

      # Point Cargo at the kellnr sidecar
      - name: Configure Cargo registry
        run: |
          mkdir -p ~/.cargo
          cat >> ~/.cargo/config.toml <<EOF
          [registries.rehearsal]
          index = "sparse+http://localhost:8000/api/v1/crates/"
          EOF
          cat >> ~/.cargo/credentials.toml <<EOF
          [registries.rehearsal]
          token = "Bearer ${{ secrets.KELLNR_TOKEN }}"
          EOF

      - name: Rehearse
        env:
          CARGO_REGISTRIES_REHEARSAL_TOKEN: ${{ secrets.KELLNR_TOKEN }}
        run: shipper rehearse --rehearsal-registry rehearsal

      - name: Upload rehearsal artifacts (always)
        if: always()
        uses: actions/upload-artifact@v7
        with:
          name: shipper-rehearsal
          path: .shipper/
          include-hidden-files: true

  publish:
    needs: rehearse
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: actions/download-artifact@v8
        with:
          name: shipper-rehearsal
          path: .shipper/
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo install shipper-cli --locked
      - name: Publish (hard gate reads .shipper/rehearsal.json)
        env:
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
        run: shipper publish --rehearsal-registry rehearsal
```

Key points:

- `rehearse` job runs against kellnr, produces `.shipper/rehearsal.json`.
- Artifact hands the receipt off to the `publish` job.
- `publish` job sets `--rehearsal-registry` so the hard gate activates
  and consults the downloaded `rehearsal.json`. If rehearsal passed
  and `plan_id` matches, publish proceeds. Otherwise, fail.

## Troubleshooting

### "rehearsal registry 'X' is not configured"

You passed `--rehearsal-registry X` or set `[rehearsal] registry = "X"`
but there's no matching `[[registries]]` entry. Add one, or pass the
registries explicitly via `--registries X,crates-io`.

### "rehearsal registry must differ from the live target"

You configured the rehearsal to point at crates.io. That defeats the
point — rehearsal is supposed to be a sandbox. Point at a different
registry.

### "rehearsal receipt is stale: plan_id mismatch"

The workspace changed between `shipper rehearse` and `shipper publish`.
Re-run `shipper rehearse` on the current workspace state.

### "no rehearsal receipt was found"

The hard gate fires when a rehearsal registry is configured but
`rehearsal.json` is missing. Either run `shipper rehearse` first, or
pass `--skip-rehearsal` to bypass (not recommended).

## What rehearsal does NOT cover (yet)

- **Install-smoke.** The current rehearsal publishes + verifies
  visibility. It does not yet run `cargo install --registry rehearsal
  <crate>` on the workspace's top crate to prove end-to-end
  resolution. That's the next follow-on under #97.
- **Consumer-workspace build.** A tiny fixture consumer crate that
  depends on the rehearsal-registry version and runs `cargo build`
  would catch `workspace-path → registry-path` resolution bugs the
  publish-only rehearsal misses. Also follow-on.

## See also

- [Inspect state, events, and receipts](inspect-state-and-receipts.md)
  — understanding the `rehearsal.json` + events the rehearsal emits.
- [Release runbook](../release-runbook.md) — the production release
  checklist; add a "rehearsal green?" step before cutting a tag.
- [#97 on the roadmap](https://github.com/EffortlessMetrics/shipper/issues/97)
  — Prove pillar tier 2.
