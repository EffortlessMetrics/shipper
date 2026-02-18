# Shipper Roadmap

This document outlines the planned development trajectory for Shipper, a reliability layer around `cargo publish` for workspace publishing. It provides detailed feature specifications, technical implementation considerations, and guidance for contributors and users.

## Table of Contents

1. [Current Release: v0.2.0](#current-release-v020)
2. [Design Principles](#design-principles)
3. [Technical Debt to Address](#technical-debt-to-address)
4. [Performance Goals](#performance-goals)
5. [Ecosystem Integration](#ecosystem-integration)
6. [Version Roadmap](#version-roadmap)
   - [v0.3.0 - User Experience](#v030---user-experience)
   - [v0.4.0 - Extensibility](#v040---extensibility)
   - [v0.5.0 - Multi-Registry](#v050---multi-registry)
   - [v1.0.0 - Stability](#v100---stability)
7. [Feature Specifications](#feature-specifications)
8. [Explicit Non-Goals](#explicit-non-goals)
9. [Contributing to the Roadmap](#contributing-to-the-roadmap)

---

## Current Release: v0.2.0

Shipper v0.2.0 is feature-complete and stable. It provides:

- **Deterministic publish planning** - Reproducible crate publish order based on dependency graph analysis
- **Preflight verification** - Comprehensive checks before publishing (finishability assessment, ownership verification, dry-run)
- **Readiness checking** - Verify crates are available on crates.io via API, index, or both methods
- **Evidence capture** - Receipts and event logs for complete audit trails
- **Parallel publishing** - Publish independent crates concurrently with wave-based execution
- **Configuration file support** - `.shipper.toml` for workspace settings with CLI flag overrides

See the [RELEASE_NOTES_v0.2.0.md](RELEASE_NOTES_v0.2.0.md) for complete details.

---

## Design Principles

These principles guide all Shipper development decisions:

### 1. Reliability Over Speed

Shipper prioritizes correct behavior and data integrity over fast execution. When in doubt, Shipper verifies, logs, and provides evidence rather than assuming success.

- **Default to safe behavior**: The default publish policy (`safe`) includes all verification steps
- **Evidence over assumptions**: Every operation captures evidence for debugging and auditing
- **Explicit over implicit**: Users must opt-in to faster but riskier behaviors

### 2. Determinism

Publishing should be reproducible across environments and time:

- **Deterministic ordering**: Publish order is computed from dependency graph with stable sorting
- **Plan IDs**: Every publish plan has a SHA256-based ID for resume validation
- **State versioning**: State and receipt files include schema versions for compatibility

### 3. Transparency

Users should understand what Shipper is doing and why:

- **Clear progress indication**: Each step's purpose and status is visible
- **Detailed logging**: Event log captures every operation with context
- **Evidence preservation**: Failed operations retain stdout/stderr for debugging

### 4. Composability

Shipper should integrate well with existing workflows:

- **Thin CLI layer**: Core logic lives in the library crate, CLI is minimal
- **Trait-based abstractions**: `StateStore`, `RegistryClient`, and `Reporter` allow customization
- **Configuration flexibility**: CLI flags, environment variables, and config files with clear precedence

### 5. Minimal External Dependencies

Shipper should be lightweight and easy to audit:

- **Few runtime dependencies**: Prefer standard library and well-audited crates
- **No runtime reflection**: All types are statically known for binary size and security
- **Explicit feature flags**: Users can disable unused functionality

---

## Technical Debt to Address

### High Priority

| Issue | Description | Impact | Approach |
|-------|-------------|--------|----------|
| Error categorization | Some errors aren't properly classified as retryable vs permanent | Unnecessary retries or premature failures | Expand `ErrorClass` enum and classification logic |
| Test coverage gaps | Some error paths lack test coverage | Potential bugs in edge cases | Add integration tests for error scenarios |
| Lock granularity | File-level locking prevents parallel workspace publishes | Limited CI parallelism | Implement directory-based locking per workspace |

### Medium Priority

| Issue | Description | Impact | Approach |
|-------|-------------|--------|----------|
| Config migration | No automatic config file migration between versions | User friction on upgrades | Add schema version checking with migration hints |
| Sparse index caching | Index is re-fetched for each readiness check | Slow readiness verification | Implement configurable caching with TTL |
| Verbose output | CLI output can be overwhelming in CI | Difficult to parse | Add machine-readable output modes |

### Lower Priority

| Issue | Description | Impact | Approach |
|-------|-------------|--------|----------|
| Documentation examples | Some edge cases lack examples | User confusion | Expand docs with scenario-based guides |
| Error messages | Some errors could be more actionable | Debugging difficulty | Review and improve error message clarity |

---

## Performance Goals

Shipper aims to balance reliability with reasonable performance:

| Metric | Current | Target | Version |
|--------|---------|--------|---------|
| Publish overhead (single crate) | ~2-5s | <3s | v0.3.0 |
| Parallel publish efficiency | ~70% | >85% | v0.4.0 |
| Readiness check latency (API) | ~500ms | <200ms | v0.3.0 |
| Memory usage (100 crates) | ~50MB | <30MB | v0.4.0 |
| Cold start time | ~1s | <500ms | v0.3.0 |
| State file size (per crate) | ~1KB | <500B | v0.5.0 |

### Optimization Priorities

1. **Parallel publishing efficiency**: Reduce coordination overhead between parallel tasks
2. **Readiness check caching**: Cache API responses to avoid redundant network calls
3. **Lazy initialization**: Defer expensive operations until needed
4. **Streaming state updates**: Write state incrementally instead of full rewrites

---

## Ecosystem Integration

### Package Manager Integration

| Manager | Status | Notes |
|---------|--------|-------|
| Cargo | **Primary** | Shipper wraps cargo publish |
| crates.io | **Supported** | Default registry |
| GitHub crates.io index | **Supported** | Via sparse protocol |
| Alternative registries | **Planned** | See v0.5.0 |

### CI/CD Integration

| Platform | Status | Template |
|----------|--------|----------|
| GitHub Actions | **Available** | `shipper ci github-actions` |
| GitLab CI | **Available** | `shipper ci gitlab` |
| Azure DevOps | **Planned** | See v0.3.0 |
| CircleCI | **Planned** | See v0.3.0 |

### Release Tooling Integration

Shipper explicitly does NOT replace release orchestration tools. Integration points:

- **cargo-release**: Use for version bumping, changelog generation, git tags
- **release-plz**: Use for automated PR-based releases
- **GitHub CLI**: Use for GitHub release creation
- **custom scripts**: Shipper emits JSON output for programmatic consumption

---

## Version Roadmap

### v0.3.0 - User Experience

**Theme**: Improve CLI usability and developer experience

**Estimated Timeline**: Q2 2026

**Focus Areas**:
- Shell completions for bash, zsh, fish, PowerShell
- Progress bars and TTY enhancement
- Enhanced dry-run output
- Additional CI templates

**Breaking Changes**: Minimal expected

---

### v0.4.0 - Extensibility

**Theme**: Enable customization and advanced use cases

**Estimated Timeline**: Q3-Q4 2026

**Focus Areas**:
- Plugin system for custom verification steps
- Cloud storage backends for StateStore
- Custom retry strategies per error type
- Webhook notifications

**Breaking Changes**: Possible config format additions

---

### v0.5.0 - Multi-Registry

**Theme**: Support multiple registries

**Estimated Timeline**: Q4 2026 - Q1 2027

**Focus Areas**:
- Multi-registry publishing in one run
- Alternative registry priority configuration
- Custom registry token management
- Registry-specific retry strategies

**Breaking Changes**: State format may need migration

---

### v1.0.0 - Stability

**Theme**: Stabilize for production use

**Estimated Timeline**: 2027

**Focus Areas**:
- Stable public API guarantees
- Comprehensive docs.rs coverage
- Formal deprecation policy
- State file format stabilization
- Receipt format versioning guarantees

**Breaking Changes**: Final breaking change window before stability

---

## Feature Specifications

### Shell Completions

| Attribute | Details |
|-----------|---------|
| **Description** | Generate shell completion scripts for bash, zsh, fish, and PowerShell |
| **User Benefit** | Enables tab completion for shipper commands and options |
| **Technical Approach** | Use `clap` built-in completion generation; add `shipper completion <shell>` subcommand |
| **Dependencies** | `clap_complete` crate (already dependency) |
| **Complexity** | Small |
| **Success Criteria** | All shipper flags complete correctly; documentation shows usage |

```bash
# Usage
shipper completion bash > /etc/bash_completion.d/shipper
shipper completion zsh > ~/.zsh/completions/_shipper
shipper completion fish > ~/.config/fish/completions/shipper.fish
```

---

### Progress Bars

| Attribute | Details |
|-----------|---------|
| **Description** | Visual progress indication for TTY environments with optional plain text fallback |
| **User Benefit** | Better visibility into publish progress, especially for large workspaces |
| **Technical Approach** | Use `indicatif` crate; detect TTY via `atty`; fallback to structured text for CI |
| **Dependencies** | `indicatif`, `atty` crates |
| **Complexity** | Small |
| **Success Criteria** | Progress bar shows during publish; respects `--no-progress` flag; CI environments get text output |

```bash
# Example output
Publishing my-crate v1.2.0 [=====>    ] 4/10 crates
```

---

### Enhanced Dry-Run

| Attribute | Details |
|-----------|---------|
| **Description** | Detailed dependency analysis in dry-run mode showing which crates depend on which |
| **User Benefit** | Better understanding of publish order and dependency relationships |
| **Technical Approach** | Extend current dry-run output with dependency graph visualization |
| **Dependencies** | None (reuse existing plan.rs logic) |
| **Complexity** | Small |
| **Success Criteria** | Shows complete dependency tree; highlights potential issues |

```bash
# Example output
Publish order: [crate-a, crate-b, crate-c, crate-d]
Dependency tree:
  crate-a (no deps)
  crate-b (depends on: crate-a)
  crate-c (depends on: crate-a)
  crate-d (depends on: crate-b, crate-c)
```

---

### Additional CI Templates

| Attribute | Details |
|-----------|---------|
| **Description** | Generate workflow snippets for Azure DevOps, CircleCI, and other CI platforms |
| **User Benefit** | Easier integration with existing CI infrastructure |
| **Technical Approach** | Add new subcommands: `shipper ci azure`, `shipper ci circleci` |
| **Dependencies** | None |
| **Complexity** | Small |
| **Success Criteria** | Templates generate valid CI configuration; include common options |

---

### Webhook Notifications

| Attribute | Details |
|-----------|---------|
| **Description** | HTTP callbacks on publish events (started, success, failure per crate) |
| **User Benefit** | Integrate with external monitoring, Slack, custom automation |
| **Technical Approach** | Add webhook configuration in `.shipper.toml`; async HTTP POST with retry |
| **Dependencies** | `reqwest` or `ureq` for HTTP client |
| **Complexity** | Medium |
| **Success Criteria** | Webhooks fire on configured events; payload includes crate info; retry on failure |

```toml
[webhook]
enabled = true
url = "https://example.com/hooks/shipper"
events = ["publish_started", "publish_success", "publish_failure"]
retry = true
```

---

### Custom Retry Strategies

| Attribute | Details |
|-----------|---------|
| **Description** | Configure retry behavior based on error type (rate limit, network, auth) |
| **User Benefit** | More intelligent retries; faster recovery from known error patterns |
| **Technical Approach** | Extend retry config with per-error-type settings; enhance error classification |
| **Dependencies** | None (existing error handling) |
| **Complexity** | Medium |
| **Success Criteria** | Different backoff for 429 vs 5xx; auth errors don't retry; config validates properly |

```toml
[retry]
max_attempts = 6
base_delay = "1s"
max_delay = "2m"

[retry.strategy]
http_429 = { base_delay = "5s", max_delay = "10m", max_attempts = 20 }
network = { base_delay = "1s", max_delay = "1m", max_attempts = 5 }
auth = { max_attempts = 1 }  # Don't retry auth errors
```

---

### State File Encryption

| Attribute | Details |
|-----------|---------|
| **Description** | Encrypt sensitive state data at rest using AES-256-GCM |
| **User Benefit** | Protect potentially sensitive publish metadata in shared environments |
| **Technical Approach** | Add encryption layer to `StateStore`; key from environment or user input |
| **Dependencies** | `ring` or `aes-gcm` crate |
| **Complexity** | Medium |
| **Success Criteria** | State files encrypted with user-provided key; graceful degradation if no key |

```bash
# Usage
shipper publish --encrypt-state
# Or via config
[state]
encrypt = true
key_env = "SHIPPER_STATE_KEY"
```

---

### Cloud Storage Backends

| Attribute | Details |
|-----------|---------|
| **Description** | Implement `StateStore` for S3, GCS, and Azure Blob storage |
| **User Benefit** | Share state across CI runners; enable distributed publishing |
| **Technical Approach** | Add trait implementations for cloud providers; support credentials via environment |
| **Dependencies** | Cloud SDK crates (`aws-sdk-s3`, `google-cloud-storage`, `azure-storage-blob`) |
| **Complexity** | Large |
| **Success Criteria** | State persists to cloud; concurrent access handled; works with major providers |

```toml
[store]
backend = "s3"
bucket = "my-workspace-shipper"
region = "us-east-1"
prefix = "releases/"
```

---

### Plugin System

| Attribute | Details |
|-----------|---------|
| **Description** | Extend Shipper with custom verification steps, pre/post hooks |
| **User Benefit** | Custom checks (security scans, license verification) without modifying Shipper |
| **Technical Approach** | Define plugin trait; support WASM or external process plugins; add `shipper plugin` subcommand |
| **Dependencies** | `wasmtime` or process execution |
| **Complexity** | Large |
| **Success Criteria** | Plugin interface stable; example plugins demonstrate use cases; sandboxed execution |

```toml
[plugins]
enabled = true

[[plugins]]
name = "security-scan"
path = "./shipper-security-plugin.wasm"
hooks = ["pre_publish", "post_publish"]
```

---

### Multi-Registry Publishing

| Attribute | Details |
|-----------|---------|
| **Description** | Publish crates to multiple registries in a single run |
| **User Benefit** | Support crate mirrors, enterprise registries, and redundancy |
| **Technical Approach** | Extend publish pipeline with registry list; maintain per-registry state |
| **Dependencies** | Alternative registry support in config |
| **Complexity** | Large |
| **Success Criteria** | Publish to crates.io + private registry; state tracks per-registry progress |

```bash
# Usage
shipper publish --registries crates-io,my-crates

# Config
[[registries]]
name = "crates-io"
default = true

[[registries]]
name = "my-crates"
url = "https://crates.mycompany.com"
token = { env = "MY_CRATES_TOKEN" }
```

---

### Alternative Registry Support

| Attribute | Details |
|-----------|---------|
| **Description** | Support publishing to registries other than crates.io |
| **User Benefit** | Enterprise users can publish to private registries |
| **Technical Approach** | Extend `RegistryClient` trait; support crates.io-compatible APIs |
| **Dependencies** | Configuration for custom registry URLs |
| **Complexity** | Medium |
| **Success Criteria** | Works with common alternatives (GitHub, Cloudsmith, self-hosted) |

---

### JSON/YAML Output Formats

| Attribute | Details |
|-----------|---------|
| **Description** | Machine-readable output for all commands |
| **User Benefit** | Parse Shipper output in scripts and tooling |
| **Technical Approach** | Add `--format json|yaml` flag to relevant commands |
| **Dependencies** | `serde_json`, `serde_yaml` crates |
| **Complexity** | Small |
| **Success Criteria** | All status commands support JSON/YAML; schema documented |

```bash
# Example output
shipper status --format json
{
  "plan_id": "abc123",
  "crates": [
    {"name": "crate-a", "version": "1.0.0", "state": "Published"}
  ]
}
```

---

### Improved Error Categorization

| Attribute | Details |
|-----------|---------|
| **Description** | Classify more errors as retryable or permanent with better detection |
| **User Benefit** | Faster failure detection; fewer unnecessary retries |
| **Technical Approach** | Enhance error parsing from cargo output; add more HTTP status code handling |
| **Dependencies** | None |
| **Complexity** | Medium |
| **Success Criteria** | All known error types classified; ambiguous cases logged for improvement |

---

### Lock File Improvements

| Attribute | Details |
|-----------|---------|
| **Description** | Implement directory-based locking for parallel workspace publishes |
| **User Benefit** | Run Shipper on multiple workspaces simultaneously |
| **Technical Approach** | Use workspace path hash for lock file location instead of global `.shipper/lock` |
| **Dependencies** | None |
| **Complexity** | Small |
| **Success Criteria** | Concurrent Shipper runs on different workspaces succeed |

---

## Explicit Non-Goals

Shipper intentionally does NOT plan to support:

| Feature | Alternative |
|---------|-------------|
| Version bumping | Use [cargo-release](https://github.com/crate-ci/cargo-release) |
| Changelog generation | Use [release-plz](https://github.com/MarcoIeni/release-plz) |
| Git tag creation | Use cargo-release |
| GitHub release creation | Use `gh` CLI or GitHub Actions |
| crate.io team management | Use `cargo owner` directly |
| Dependency updates | Use cargo's built-in commands |

**Shipper focuses on reliable publishing, not release orchestration.**

---

## Contributing to the Roadmap

### How Features Are Prioritized

Roadmap items are prioritized based on:

1. **Community feedback** - Feature requests and use cases from GitHub Issues
2. **Ecosystem trends** - Changes to crates.io, Cargo, or Rust tooling
3. **Maintenance burden** - Complexity vs. value trade-offs
4. **Strategic alignment** - Fit with Shipper's design principles

### Suggesting a Feature

To suggest a feature:

1. Check existing [GitHub Issues](https://github.com/cmrigney/shipper/issues)
2. Open a new issue with the `enhancement` label
3. Describe your use case and expected behavior
4. Include: User story, technical approach (if known), priority justification

### Implementing a Feature

If you'd like to implement a roadmap item:

1. Comment on the GitHub issue to express interest
2. Review the [CONTRIBUTING.md](CONTRIBUTING.md) for development guidelines
3. Propose an implementation plan in a draft PR
4. Ensure tests and documentation are included

---

## Version History

| Version | Status | Theme | Notes |
|---------|--------|-------|-------|
| v0.2.0 | **Current** | Evidence & Verification | Parallel publishing, configuration files, readiness verification |
| v0.1.0 | Released | Core Functionality | Initial release with basic publish workflow |

---

## Appendix: Complexity Guidelines

| Rating | Description | Typical Effort |
|--------|-------------|----------------|
| **Small** | Can be implemented in < 1 week; affects single module | 2-5 days |
| **Medium** | Requires architectural consideration; 1-2 weeks | 1-2 weeks |
| **Large** | Major feature requiring design; 1+ months | Several weeks to months |

Complexity estimates assume:
- Familiarity with Shipper codebase
- Code review and testing requirements
- Documentation updates included
