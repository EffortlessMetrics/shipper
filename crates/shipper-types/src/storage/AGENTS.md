# AGENTS.md

Use this file with [CLAUDE.md](./CLAUDE.md) before making changes in this directory.

# Module: `shipper_types::storage`

**Crate:** `shipper-types` (public)
**Single responsibility:** Storage backend configuration types — the stable contract embedders use to declare their storage choice.
**Was:** Part of the standalone `shipper-storage` crate (split in this PR).

## Public API
- `StorageType` — Enum: `File | S3 | Gcs | Azure`
- `CloudStorageConfig` — Configuration for any storage backend (bucket, region, base_path, credentials, etc.)
- `ParseStorageTypeError` — Error returned by `FromStr for StorageType`
- `ValidateStorageConfigError` — Error returned by `CloudStorageConfig::validate`

## Why this lives in shipper-types
These are pure data — no I/O, no policy decisions. Embedders need to express "use this storage backend" through the stable contract crate. The runtime backend behavior lives in `shipper::ops::storage` and is internal/unfinished (only filesystem is implemented today).

Errors here use plain structs rather than `anyhow::Error` so `shipper-types` stays free of `anyhow`.

