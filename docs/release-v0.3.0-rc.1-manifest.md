# Release Manifest — shipper v0.3.0-rc.1

This manifest is the output of **Phase 8 (package-truth validation)** of the
decrating effort. It captures the topological publish order, the verified
per-crate tarball contents, and the rate-limit and resume discipline required
for the first-publish wave.

All 12 surviving crates are **currently unpublished** on crates.io. This is the
first release train; every crate on this list is a brand-new package.

## Topological publish order

The order below was verified by inspecting the `[dependencies]` section of each
crate's `Cargo.toml` on this branch. Each tier depends only on tiers above it,
so tiers may be published serially; crates within a tier have no inter-tier
edges and therefore also have no intra-tier edges.

### Tier 1 — Leaves (no sibling deps on other shipper crates)

1. `shipper-duration`
2. `shipper-retry`
3. `shipper-encrypt`
4. `shipper-output-sanitizer`
5. `shipper-cargo-failure`
6. `shipper-sparse-index`
7. `shipper-webhook`

Note: the Phase 8 plan anticipated `shipper-retry` depending on
`shipper-duration` and `shipper-webhook` depending on `shipper-types`. Neither
edge exists in the current workspace; both are leaves.

### Tier 2 — One hop from leaves

8. `shipper-types` — depends on `shipper-encrypt`, `shipper-webhook`,
   `shipper-retry`, `shipper-duration`

### Tier 3 — Depend on `shipper-types`

9. `shipper-registry` — depends on `shipper-sparse-index`, `shipper-types`
10. `shipper-config` — depends on `shipper-types`, `shipper-encrypt`,
    `shipper-webhook`, `shipper-retry`

### Tier 4 — Core library

11. `shipper` — depends on all of Tiers 1–3

### Tier 5 — CLI binary

12. `shipper-cli` — depends on `shipper`, `shipper-duration`

## Per-crate package truth (this branch)

All 12 crates pass `cargo package --list -p <crate>` (manifest validity).

Seven leaf crates pass `cargo package -p <crate>` (full tarball compile) and
`cargo publish --dry-run -p <crate>` cleanly.

The five dependent crates (`shipper-types`, `shipper-registry`,
`shipper-config`, `shipper`, `shipper-cli`) fail the full `cargo package` verify
step with `error: no matching package named <dep> found`. This is expected and
is the "brand-new crate chain" problem: the verify step resolves dependencies
against crates.io, and the upstream workspace crates are not yet published.
The manifests themselves are correct; the failure is a registry-visibility
artefact that disappears once the prior tier is live.

### Tarball contents (successful `cargo package` runs)

| Crate                     | Size      | Files |
|---------------------------|-----------|-------|
| shipper-duration          | 13.2 KB   | 33    |
| shipper-retry             | 16.2 KB   | 30    |
| shipper-encrypt           | 23.9 KB   | 45    |
| shipper-output-sanitizer  | 12.5 KB   | 25    |
| shipper-cargo-failure     | 18.0 KB   | 59    |
| shipper-sparse-index      | 12.7 KB   | 16    |
| shipper-webhook           | 32.6 KB   | 36    |

### File counts from `cargo package --list` (all 12)

| Crate                     | Files |
|---------------------------|-------|
| shipper-duration          | 33    |
| shipper-retry             | 30    |
| shipper-encrypt           | 45    |
| shipper-output-sanitizer  | 25    |
| shipper-cargo-failure     | 59    |
| shipper-sparse-index      | 16    |
| shipper-webhook           | 36    |
| shipper-types             | 105   |
| shipper-registry          | 38    |
| shipper-config            | 56    |
| shipper                   | 429   |
| shipper-cli               | 162   |

Every tarball contains:

- `Cargo.toml` (auto-generated from workspace)
- `Cargo.toml.orig` (original manifest)
- `README.md`
- `src/lib.rs` or `src/main.rs`

No unexpected files (`.env`, `.vscode/`, `.idea/`, credential material, IDE
configs) were found in any listing.

## [package] metadata audit

All 12 crates carry the full crates.io-recommended metadata set:

- `description`
- `license` (via `workspace.package`, `MIT OR Apache-2.0`)
- `repository`
- `keywords` (≤ 5)
- `categories` (≤ 5)
- `documentation`

One casing inconsistency was fixed during Phase 8:
`shipper-retry`'s `repository` URL was lowercase
(`github.com/effortlessmetrics/shipper`); it was normalised to
`EffortlessMetrics` to match the rest of the workspace.

## Rate-limit plan

crates.io enforces the following limits on new crate publication:

- **5-crate burst** allowed on a fresh session
- After the burst, **1 new crate per 10 minutes** until the limit resets

With 12 brand-new crates and the burst budget used on Tier 1's first five
publishes, the remaining seven crates are rate-limited:

| Minute | Action                                              |
|--------|-----------------------------------------------------|
| 0      | Publish Tier 1 crates 1–5 (burst)                   |
| 10     | Publish Tier 1 crate 6 (`shipper-sparse-index`)     |
| 20     | Publish Tier 1 crate 7 (`shipper-webhook`)          |
| 30     | Publish Tier 2 (`shipper-types`)                    |
| 40     | Publish Tier 3 crate 9 (`shipper-registry`)         |
| 50     | Publish Tier 3 crate 10 (`shipper-config`)          |
| 60     | Publish Tier 4 (`shipper`)                          |
| 70     | Publish Tier 5 (`shipper-cli`)                      |

Minimum wall-clock: **~70–80 minutes** for the full train, not counting
post-publish verification.

Between each publish, readiness verification (sparse-index or registry-API
lookup) must succeed before the next publish begins. That's the whole point of
the `verify` step in the shipper publish engine — use it.

## `.shipper/` state persistence

This release MUST be driven by `shipper publish` itself (dogfooding). Requirements:

- `.shipper/state.json` persists after every single crate publish
- `.shipper/events.jsonl` captures every state transition
- `.shipper/receipt.json` captures evidence (stdout/stderr, exit codes, git SHA)
- `.shipper/lock` held for the duration of the run; concurrent publishes must
  be rejected

If the train is interrupted (network blip, rate-limit surprise, human pause),
resume rules apply:

- Resume MUST validate plan ID against the saved state — any workspace
  modification since the run started invalidates the plan and aborts resume
- Already-published crates MUST be skipped (registry visibility confirmed, not
  just state-file flag)
- Resume continues from the first pending or failed crate

## Resume discipline

1. Never rerun `cargo publish` manually on a crate that the state file marks as
   published. Trust the state.
2. If the state file is lost, recover by querying crates.io for each crate's
   presence at version `0.3.0-rc.1` and rebuild the ledger from that ground
   truth. Only then rerun shipper.
3. If a publish fails with a `Permanent` error class (auth, version conflict),
   do NOT retry. Fix the cause, bump version if necessary, restart.
4. `Ambiguous` errors (upload may have succeeded) require an out-of-band check
   of the registry before deciding to retry or skip.

## How the workflow executes the train

The release workflow at `.github/workflows/release.yml` dogfoods Shipper:
the crates.io publish train is driven by `shipper plan` → `shipper preflight`
→ `shipper publish`, not raw `cargo publish`.

### `publish-crates-io` job (tag push, `v*.*.*`)

1. Install Rust stable and cache `~/.cargo`.
2. `cargo build --release -p shipper-cli`, then install `target/release/shipper`
   to `/usr/local/bin/shipper` (the binary that will drive the train is the
   one we just built from this tag).
3. `shipper plan --registry crates-io --state-dir .shipper --format json`
   writes the plan summary into `.shipper/plan.txt` (and the engine also
   persists its internal plan artefacts under `.shipper/`).
4. `.shipper/` is uploaded as an artifact (`shipper-state-plan`) **before**
   preflight runs, so the plan survives catastrophic job failure.
5. `shipper preflight --registry crates-io --state-dir .shipper --policy safe`
   runs git-clean / registry-reachability / version-not-taken checks.
   `.shipper/` is uploaded again (`shipper-state-preflight`). On failure the
   workflow fails fast before any publish happens.
6. `shipper publish` runs the real train with these flags:
    - `--policy safe`  (verify + strict)
    - `--verify-mode package`  (per-crate post-publish verify)
    - `--readiness-method both`  (sparse-index **and** API)
    - `--readiness-timeout 15m`, `--verify-timeout 10m`
    - `--max-attempts 12`, `--base-delay 10s`, `--max-delay 15m`,
      `--retry-strategy exponential`
7. On success, `cargo search <crate>` is run against every published crate
   to confirm visibility from a fresh resolver.
8. `.shipper/` is uploaded a final time as `shipper-state-final` (retention
   90 days). The GitHub Release is then created, attaching platform binaries
   plus a tarball of the final `.shipper/` state as publish evidence.

### Rate-limit handling for the first-publish train

crates.io allows a 5-crate burst for new crates and then 1 new crate per
10 minutes (see §"Rate-limit plan" above). The workflow does **not** hard-code
tier batching or sleeps; the shipper engine handles rate-limit 429s as
retryable errors with exponential backoff, and the post-publish readiness
check (both sparse-index **and** API because `--readiness-method both`) blocks
the next publish until the previous crate is visible. `--max-delay 15m` gives
the backoff loop enough headroom to wait out the 10-minute new-crate window.
The whole train completes in roughly 70–90 minutes inside the single runner.

If the job hits the 180-minute timeout (or the runner dies), the `.shipper/`
artifact is still uploaded and the dedicated `release-resume` workflow_dispatch
job downloads it and runs `shipper resume`.

### `release-rehearse` (workflow_dispatch)

Runs `shipper plan --verbose` + `shipper preflight --skip-ownership-check`
against the requested ref and uploads `.shipper/`. Use this before tagging
to verify nothing is broken. No publishing happens.

### `release-resume` (workflow_dispatch)

Accepts an `artifact_run_id` input, downloads the prior `shipper-state-final`
artifact into `.shipper/`, and runs `shipper resume` with the same policy
flags as `publish`. The plan-ID check in `shipper resume` is the guardrail:
if the workspace changed between runs, resume aborts rather than produce
a desynced ledger.

## Known deferred work

- **Non-leaf `cargo publish --dry-run`**: will not pass until Tier 1 is live on
  crates.io. This is a property of crates.io resolver behaviour, not a manifest
  defect. Re-run dry-run after each tier ships to confirm the next tier is
  clean before actual publish.
- **docs.rs build**: documentation will build on docs.rs only after each crate
  is published. The `documentation = "https://docs.rs/..."` URL in every
  manifest is a forward reference.
