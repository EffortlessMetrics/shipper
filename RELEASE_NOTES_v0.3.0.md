# Release Notes - Shipper v0.3.0-rc.1

## Overview

Shipper v0.3.0-rc.1 is a significant upgrade that brings massive improvements to user experience, performance, and enterprise-grade features. This release transitions Shipper into a fully modular architecture and adds support for complex multi-registry workflows.

## Key Features

### 🌐 Multi-Registry Publishing
You can now publish your workspace to multiple registries in a single execution. Shipper handles the orchestration, state persistence, and evidence capture for each registry independently.
```bash
shipper publish --registries crates-io,internal-mirror
```

### ⚡ High-Performance Sparse Index Caching
Visibility polling is now faster and more efficient thanks to ETag-based disk caching. Shipper avoids redundant downloads of index fragments, reducing bandwidth and minimizing the risk of being throttled by registry APIs.

### 🔄 Selective Resumability
Interrupted publishes can now be resumed from any specific package, not just the last failed one. This gives you total control over the recovery process.
```bash
shipper publish --resume-from my-critical-crate
```

### 🩺 Advanced Diagnostics (shipper doctor)
The `doctor` command has been expanded to perform deep health checks on your environment, including:
- Registry API reachability pings
- State directory permission validation
- Git context detection
- Sparse index base verification

### 🤖 CI/CD Enhancements
- **Quiet Mode**: Use `--quiet` to get clean, minimal logs in CI environments.
- **Improved Progress**: Progress reporting now automatically optimizes for non-TTY logs.
- **New Templates**: Built-in generators for Azure DevOps and CircleCI.

## Technical Improvements
- **Workspace-Aware Locking**: Lock files are now hashed by workspace path, preventing global collisions and allowing parallel publishes of different workspaces.
- **Atomic State Writes**: Improved data integrity through atomic filesystem operations for all state and lock files.
- **Modular Architecture**: Shipper is organized as a small set of public crates (`shipper`, `shipper-cli`, `shipper-config`, `shipper-types`, `shipper-registry`, and a handful of focused leaf crates). An earlier RC split every concern into its own microcrate; those layers have since been consolidated back into module folders inside `shipper`, `shipper-config`, and `shipper-cli` (see `docs/architecture.md` and the decrating plan).

## Installation
```bash
cargo install shipper-cli --version 0.3.0-rc.1
```

## Contributors
Thanks to everyone who contributed to this release!
