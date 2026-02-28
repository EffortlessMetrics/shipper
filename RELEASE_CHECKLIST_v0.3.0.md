# Release Checklist for v0.3.0-rc.1

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

- [ ] Commit all changes with message "release: v0.3.0-rc.1"
- [ ] Tag the commit: `git tag -a v0.3.0-rc.1 -m "Release v0.3.0-rc.1"`
- [ ] Push commit and tag: `git push origin main --tags`
- [ ] Publish to crates.io (dry-run first):
  ```bash
  cargo publish -p shipper-types --dry-run
  cargo publish -p shipper-schema --dry-run
  cargo publish -p shipper-duration --dry-run
  cargo publish -p shipper-retry --dry-run
  cargo publish -p shipper-output-sanitizer --dry-run
  cargo publish -p shipper-sparse-index --dry-run
  cargo publish -p shipper-environment --dry-run
  cargo publish -p shipper-events --dry-run
  cargo publish -p shipper-policy --dry-run
  cargo publish -p shipper-lock --dry-run
  cargo publish -p shipper-process --dry-run
  cargo publish -p shipper-encrypt --dry-run
  cargo publish -p shipper-auth --dry-run
  cargo publish -p shipper-progress --dry-run
  cargo publish -p shipper-cargo-failure --dry-run
  cargo publish -p shipper-cargo --dry-run
  cargo publish -p shipper-webhook --dry-run
  cargo publish -p shipper-registry --dry-run
  cargo publish -p shipper-execution-core --dry-run
  cargo publish -p shipper-state --dry-run
  cargo publish -p shipper-plan --dry-run
  cargo publish -p shipper-store --dry-run
  cargo publish -p shipper-config --dry-run
  cargo publish -p shipper-config-runtime --dry-run
  cargo publish -p shipper-engine-parallel --dry-run
  cargo publish -p shipper --dry-run
  cargo publish -p shipper-cli --dry-run
  ```

## Post-Release

- [ ] Create GitHub release with release notes
- [ ] Verify `cargo install shipper-cli` works
- [ ] Monitor for issues
