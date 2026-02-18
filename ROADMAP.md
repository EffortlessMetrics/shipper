# Shipper Roadmap

This document outlines the planned development trajectory for Shipper.

## Current Release: v0.2.0

Shipper v0.2.0 is feature-complete and stable. It provides:

- **Deterministic publish planning** - Reproducible crate publish order
- **Preflight verification** - Catch issues before publishing
- **Readiness checking** - Verify crates are available on crates.io (API, index, or both)
- **Evidence capture** - Receipts and event logs for audit trails
- **Parallel publishing** - Publish independent crates concurrently
- **Configuration file support** - `.shipper.toml` for workspace settings

See the [RELEASE_NOTES_v0.2.0.md](RELEASE_NOTES_v0.2.0.md) for details.

---

## Near-Term (v0.3.0)

### Under Consideration

| Feature | Description | Status |
|---------|-------------|--------|
| Shell completions | Generate bash, zsh, fish, powershell completion scripts | Planned |
| Progress bars | TTY detection with visual progress indication | Planned |
| Enhanced dry-run | Detailed dependency analysis in dry-run mode | Planned |
| CI templates | Additional templates for Azure DevOps, CircleCI | Planned |

### Community Input Needed

- Alternative registry support priorities
- Output format preferences (JSON, YAML, TOML)
- Integration with existing release tooling

---

## Medium-Term (v0.4.0 - v0.5.0)

### Potential Features

| Feature | Description |
|---------|-------------|
| Webhook notifications | Callbacks on publish events |
| Custom retry strategies | Per-error-type retry configuration |
| Multi-registry publishing | Publish to multiple registries in one run |
| State file encryption | Encrypt sensitive state data |

### Architectural Improvements

- Cloud storage backends for `StateStore` trait
- Plugin system for custom verification steps
- Improved error categorization and recovery

---

## Long-Term (v1.0.0)

### Goals

- Stable public API guarantees for library consumers
- Comprehensive docs.rs coverage
- Formal deprecation policy
- State file format stabilization
- Receipt format versioning guarantees

### Breaking Changes Under Consideration

Any breaking changes will be communicated in advance with migration guides.

---

## Explicit Non-Goals

Shipper intentionally does NOT plan to support:

| Feature | Alternative |
|---------|-------------|
| Version bumping | Use [cargo-release](https://github.com/crate-ci/cargo-release) |
| Changelog generation | Use [release-plz](https://github.com/MarcoIeni/release-plz) |
| Git tag creation | Use cargo-release |
| GitHub release creation | Use `gh` CLI or GitHub Actions |

**Shipper focuses on reliable publishing, not release orchestration.**

---

## Contributing to the Roadmap

Roadmap items are prioritized based on:

1. **Community feedback** - Feature requests and use cases
2. **Ecosystem trends** - Changes to crates.io or Cargo
3. **Maintenance burden** - Complexity vs. value trade-offs

To suggest a feature:
1. Check existing [GitHub Issues](https://github.com/cmrigney/shipper/issues)
2. Open a new issue with the `enhancement` label
3. Describe your use case and expected behavior

---

## Version History

| Version | Status | Notes |
|---------|--------|-------|
| v0.2.0 | **Current** | Parallel publishing, configuration files, readiness verification |
| v0.1.0 | Released | Initial release with basic publish workflow |