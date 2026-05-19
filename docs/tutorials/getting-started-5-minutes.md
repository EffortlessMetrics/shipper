# Tutorial: Getting to release confidence in five minutes

This path gets a maintainer from a fresh install to a decision-grade local
release check. It does not publish. Use it when you want to know what Shipper
will do and which proof gaps remain before the first irreversible command.

## What you'll need

- Rust 1.95 or newer.
- A Rust workspace with publishable crates.
- A clean git working tree, unless you intend to pass `--allow-dirty`.
- A crates.io token if you want ownership and authenticated registry checks.

## 1. Install Shipper

After the stable `0.4.0` release is published, the user-facing package is
`shipper`:

```bash
cargo install shipper --locked
shipper --version
```

If you are validating this repository from a checkout before release:

```bash
cargo install --path crates/shipper --locked
shipper --help
```

## 2. Ask Doctor for local blockers

```bash
cd /path/to/your/workspace
shipper doctor
```

Treat blocked findings as setup work before planning a publish. Common first
fixes are logging in to crates.io, cleaning the git tree, or adding missing
package metadata.

## 3. Generate the publish plan

```bash
shipper plan
```

Confirm the publishable crates, skipped crates, and dependency-ordered publish
sequence match your intent. The `plan_id` is the release fingerprint Shipper
uses to prevent resuming against a different workspace state.

## 4. Run preflight

Before preflight, you can compare local package versions to the registry:

```bash
shipper status
```

Use this read-only check when you want an early signal that a version already
exists on the target registry.

```bash
shipper preflight
```

Read the finishability result as a release decision:

| Result | Meaning | Next action |
|---|---|---|
| `Proven` | Local packaging and registry checks passed for the configured policy. | You can proceed to `shipper publish` if the version and changelog are already final. |
| `NotProven` | Nothing definitive failed, but at least one proof cannot be completed locally. | Review the listed gaps, then decide whether to publish, add rehearsal proof, or change policy. |
| `Failed` | A blocker was found before publish. | Fix the finding and rerun `shipper preflight`. |

## 5. Stop before the irreversible step

Do not run `shipper publish` until the versioning, changelog, tag, release
approval, and proof gaps are intentionally settled. If you do proceed, Shipper
writes the release evidence under `.shipper/`, including state, events, and the
final receipt.

Useful follow-up commands:

```bash
shipper publish
shipper resume
shipper inspect-events
shipper inspect-receipt
```

## Evidence to keep

After this five-minute path, keep the terminal output from:

- `shipper doctor`
- `shipper plan`
- `shipper status`
- `shipper preflight`

When JSON output is required for CI or an internal developer portal, prefer the
command's `--format json` option where available. The first-run decision path
now supports JSON for `doctor`, `plan`, `status`, `preflight`, `publish`, and
`resume`.
