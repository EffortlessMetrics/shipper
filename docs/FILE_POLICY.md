# Non-Rust File Policy

This document describes the policy for non-Rust files in the `shipper` repository. The authoritative allowlists are in `policy/`; this document explains the rationale and operating rules.

## Goals

1. Make every non-Rust file present by deliberate receipt, not by accident.
2. Provide specific policies for high-risk surfaces (workflows, executables, generated files, network-touching scripts).
3. Preserve visibility into the release workflow's shell behavior and Trusted Publishing OIDC handling.
4. Prevent opaque `**` globs from hiding security-sensitive changes.

## Non-Rust Surfaces in `shipper`

| Surface | Location | Policy file |
|---|---|---|
| GitHub Actions workflows | `.github/workflows/*.yml` | `policy/workflow-allowlist.toml` |
| Dependabot config | `.github/dependabot.yml` | `policy/workflow-allowlist.toml` |
| Codecov config | `codecov.yml` | `policy/non-rust-allowlist.toml` |
| Cargo deny config | `deny.toml` | `policy/dependency-surface-allowlist.toml` |
| Toolchain file | `rust-toolchain.toml` | `policy/non-rust-allowlist.toml` |
| Clippy config | `clippy.toml` | `policy/non-rust-allowlist.toml` |
| Documentation | `README.md`, `CHANGELOG.md`, `docs/**` | `policy/non-rust-allowlist.toml` |
| Fuzz harness | `fuzz/**` | `policy/non-rust-allowlist.toml` |
| BDD features | `features/**` | `policy/non-rust-allowlist.toml` |
| CI templates | `templates/**` | `policy/non-rust-allowlist.toml` |
| Snapshot files | `**/*.snap` | `policy/generated-allowlist.toml` |
| Release snippets | `RELEASE_*.md` | `policy/non-rust-allowlist.toml` |
| Agent docs | `AGENTS.md`, `GEMINI.md`, `CLAUDE.md` | `policy/non-rust-allowlist.toml` |

## Workflow Policy (High Risk)

The release workflow (`.github/workflows/release.yml`) is operationally critical. It:

- Builds `shipper` from source.
- Runs `shipper plan`, `preflight`, `publish`, and `resume`.
- Mints and consumes Trusted Publishing (OIDC) tokens.
- Uploads `.shipper/` state artifacts.
- Verifies registry visibility.
- Builds and uploads binary release artifacts.

Changes to the release workflow must be reviewed with extra scrutiny. The `policy/workflow-allowlist.toml` must not use a blanket `.github/**` allow — each workflow must be listed explicitly with the process/network behavior it enables.

### Process Behavior

The `policy/process-allowlist.toml` receipts which shell commands and subprocesses each workflow is permitted to invoke. The release workflow's permitted processes include: `cargo`, `rustup`, `shipper`, `gh`, `tar`, `sha256sum`.

### Network Behavior

The `policy/network-allowlist.toml` receipts which external endpoints each workflow may contact. The release workflow's permitted endpoints include: `crates.io`, `static.crates.io`, GitHub Actions OIDC endpoint, GitHub API.

## Generated Files

Snapshot files and the no-panic baseline are machine-generated. They are listed in `policy/generated-allowlist.toml` and marked `linguist-generated=true` in `.gitattributes`. Generated files must not be edited by hand; regeneration commands are documented in the relevant policy docs.

## Executable Files

Shell scripts and executable files must be listed in `policy/executable-allowlist.toml` with the reason they are executable. No file should become executable without a policy receipt.

## Dependency Surfaces

`deny.toml` (cargo-deny configuration), `Cargo.lock`, and any dependency metadata files are receipted in `policy/dependency-surface-allowlist.toml`. Changes to these files are reviewed with supply-chain awareness.

## Commands

```bash
# Inventory all non-Rust files in the repo.
cargo xtask non-rust inventory

# Propose initial allowlist entries for unreceipted files.
cargo xtask non-rust propose

# Check all files against their allowlists (advisory mode — no CI failure).
cargo xtask check-file-policy --mode advisory

# Check all files against their allowlists (blocking mode — fails if unreceipted).
cargo xtask check-file-policy --mode blocking-allowlist

# Specific surface checks.
cargo xtask check-generated
cargo xtask check-executable-files
cargo xtask check-dependency-surfaces
cargo xtask check-workflow-surfaces
cargo xtask check-process-policy
cargo xtask check-network-policy

# Full policy report.
cargo xtask policy-report
```

## Rollout

The file policy checker starts in advisory mode (PR 9). It is promoted to blocking-allowlist mode in the release dry-run proof (PR 15) once the initial inventory is complete and reviewed.
