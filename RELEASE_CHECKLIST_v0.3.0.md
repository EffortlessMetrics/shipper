# Release Checklist for v0.3.0-rc.1

> **Decrating note:** The following crates have been absorbed into
> `shipper` / `shipper-config` / `shipper-cli` as module folders and no
> longer need to be published separately:
> `shipper-lock`, `shipper-process`, `shipper-levels`, `shipper-chunking`,
> `shipper-policy`, `shipper-config-runtime`, `shipper-plan`, `shipper-store`,
> `shipper-events`, `shipper-state`.
> In-flight absorptions (`shipper-auth`, `shipper-environment`, `shipper-git`,
> `shipper-storage`, `shipper-engine-parallel`, `shipper-progress`) may also
> be removed from the publish order before v0.3.0 GA. The final publish
> order will be codified in Phase 8 of the decrating plan.

## Pre-Release Tasks

- [x] Run `cargo test --workspace --all-features` — all passing
- [x] Run `cargo clippy --workspace --all-features -- -D warnings` — clean
- [x] Verify `Cargo.toml` workspace version is `0.3.0-rc.1`
- [x] Update `CHANGELOG.md` with 0.3.0-rc.1 entry
- [x] Update `ROADMAP.md` current version
- [x] Create `RELEASE_NOTES_v0.3.0-rc.1.md`
- [x] Verify `--help` output reflects all new flags
- [x] Test `shipper completion` for at least one shell
- [x] Test `shipper doctor` in a real workspace
- [x] Verify multi-registry state segregation manually or via integration test

## Release Execution

The release is driven by `.github/workflows/release.yml`, which dogfoods
Shipper itself (`shipper plan` → `shipper preflight` → `shipper publish`).

### Pre-tag rehearsal

- [ ] Trigger the `release-rehearse` workflow_dispatch job from the ref that
      is about to be tagged. It runs `shipper plan --verbose` and
      `shipper preflight` with no publishing.
- [ ] Download the `shipper-rehearse-<run_id>` artifact and review
      `.shipper/plan.txt`. Confirm the topological order matches
      `docs/release-v0.3.0-rc.1-manifest.md`:
      `shipper-duration, shipper-retry, shipper-encrypt,
      shipper-output-sanitizer, shipper-cargo-failure, shipper-sparse-index,
      shipper-webhook, shipper-types, shipper-registry, shipper-config,
      shipper, shipper-cli`.

### Tag & release

- [ ] Commit all changes with message `release: v0.3.0-rc.1`.
- [ ] Tag the commit: `git tag -a v0.3.0-rc.1 -m "Release v0.3.0-rc.1"`.
- [ ] Push commit and tag: `git push origin main --tags`.
- [ ] The `v*.*.*` tag push triggers `.github/workflows/release.yml`:
    - `msrv-gate` + `build-binaries` run in parallel.
    - `publish-crates-io` runs after `msrv-gate`:
      `shipper plan` → upload `.shipper/` → `shipper preflight`
      → upload `.shipper/` → `shipper publish` → `cargo search` verification
      → upload final `.shipper/` state.
    - `create-release` runs only after `publish-crates-io` succeeds and
      attaches platform binaries + the final `.shipper/` state tarball.
- [ ] Do **not** run `cargo publish` by hand. The engine handles rate-limit
      backoff, readiness checks, and state persistence; manual publishes
      desync the ledger.

### If the publish train is interrupted

- [ ] Identify the failed run ID (GitHub Actions UI).
- [ ] Confirm the `shipper-state-final` (or `shipper-state-preflight`)
      artifact was uploaded by that run.
- [ ] Trigger the `release-resume` workflow_dispatch job with:
    - `mode = resume`
    - `ref = <the tag that failed>` (MUST be identical; plan-ID check
       guards against workspace drift)
    - `artifact_run_id = <failed run ID>`
- [ ] Shipper skips already-published crates and continues from the first
      pending/failed entry.

## Post-Release

- [ ] GitHub Release is created automatically after publish succeeds; verify
      it lists all platform binaries and the `shipper-release-state.tar.gz`
      evidence bundle.
- [ ] Verify `cargo install shipper-cli` works from the published crate.
- [ ] Monitor for issues.
