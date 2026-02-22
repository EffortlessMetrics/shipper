# Release Checklist for v0.2.0

## Pre-Release Tasks

- [x] Run `cargo test --workspace` — all passing (273 lib + 16 CLI unit + 22 E2E + 5 other)
- [x] Run `cargo clippy --workspace -- -D warnings` — clean
- [x] Run `cargo fmt --check` — clean
- [x] Update version to 0.2.0 in workspace Cargo.toml
- [x] Update CHANGELOG.md with v0.2.0 entry (includes parallel publishing)
- [x] Verify CI templates are correct

## Code Quality Checks

- [x] All clippy warnings resolved
- [x] Code formatting passes (`cargo fmt --check`)
- [x] All tests pass (316 total: 273 lib, 16 CLI unit, 22 CLI E2E, 5 other)
- [x] No dead code warnings
- [x] No unused dependencies

## Crate Metadata (crates.io readiness)

- [x] `description` set for both crates
- [x] `repository` URL set
- [x] `documentation` URL set (docs.rs)
- [x] `homepage` URL set
- [x] `keywords` set (cargo, publish, workspace, registry, ci)
- [x] `categories` set (development-tools::cargo-plugins)
- [x] `license` set (MIT OR Apache-2.0)
- [x] `rust-version` set (MSRV 1.92)

## Documentation

- [x] CHANGELOG.md is up to date with v0.2.0 changes
- [x] README.md reflects all features including parallel publishing
- [x] Documentation in `docs/` is current (configuration, preflight, readiness, failure-modes)
- [x] Migration guide is clear and complete (in RELEASE_NOTES and CHANGELOG)
- [x] Library rustdoc has pipeline overview and key type references

## CI/CD

- [x] GitHub Actions CI workflow created (.github/workflows/ci.yml)
- [x] CI subcommand names fixed (github-actions, gitlab)
- [x] CI templates available in templates/ directory

## Bug Fixes Applied

- [x] Fixed `shipper ci` subcommand names (git-hub-actions → github-actions, git-lab → gitlab)
- [x] Fixed E2E test mock server to handle multiple URL patterns
- [x] Fixed E2E test request counts for preflight tests (version_exists + check_new_crate)
- [x] Fixed dry-run fake cargo detection for Windows (--dry-run arg position)
- [x] Added JSON format support for `inspect-receipt` command
- [x] Made `--format` flag global so it works after subcommands

## Release Preparation

- [ ] Commit all changes
- [ ] Merge to main
- [ ] Tag the release: `git tag -a v0.2.0 -m "Release v0.2.0"`
- [ ] Push the tag: `git push origin v0.2.0`

## Publishing

- [ ] Publish to crates.io:
  ```bash
  cargo publish -p shipper
  cargo publish -p shipper-cli
  ```
- [ ] Verify packages appear on crates.io
- [ ] Test installation: `cargo install shipper-cli`

## Post-Release

- [ ] Create GitHub release with release notes
- [ ] Monitor for issues and feedback
