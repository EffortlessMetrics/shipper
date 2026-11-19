# Module: `crate::ops::cargo`

**Layer:** ops (layer 1, bottom)
**Single responsibility:** Shell out to `cargo metadata` and `cargo publish` and return the captured/truncated output.
**Was:** standalone crate `shipper-cargo` (absorbed during decrating Phase 2).

## Public-to-crate API

Re-exported at `shipper::cargo` (preserved backward compatibility with the old
`shipper-cargo` public surface).

- `CargoOutput` — value type: exit code, stdout/stderr tails, duration, timed-out flag.
- `cargo_publish(workspace_root, package, registry, allow_dirty, no_verify, output_lines, timeout)` — spawn `cargo publish -p <pkg>` with optional wall-clock timeout.
- `cargo_publish_dry_run_workspace` / `cargo_publish_dry_run_package` — dry-run variants.
- `load_metadata(manifest_path)` — invokes `cargo metadata`; used by `crate::plan`.
- `WorkspaceMetadata` — thin wrapper around `cargo_metadata::Metadata` with helpers (`publishable_packages`, `topological_order`, `workspace_members`, etc.).
- `PackageInfo` — serializable package summary.
- `is_valid_package_name(name)` — crates.io naming rule check.
- `workspace_member_names(&metadata)` — convenience.
- `get_version(manifest_path)` / `get_package_name(manifest_path)` — root-package introspection helpers.
- `pub use shipper_output_sanitizer::redact_sensitive;` — re-export; callers that log cargo output should funnel it through here.

## Invariants & gotchas

- **`SHIPPER_CARGO_BIN` override.** `cargo_program()` returns `$SHIPPER_CARGO_BIN` if set (used by tests to point at fake cargo binaries), else `"cargo"`. An empty string env var is NOT treated as unset — it is passed through verbatim.
- **Timeout and cleanup are separate.** On Linux/macOS, publication uses nonblocking
  reads, an invocation-owned process group, a 10ms quiet poll, and one second for
  group shutdown/reap/drain after exit or the execution deadline. An execution
  timeout retains `timed_out: true` and `exit_code: -1`; early exit preserves its
  result. The stderr annotation survives only when complete retained tails
  include it. The internal wrapper carries typed `cleanup_failure`; public
  `cargo_publish` returns an error if cleanup is incomplete because the legacy
  `CargoOutput` shape cannot express a safe retry hold. Other platforms retain
  the earlier direct-child polling/reader behavior.
- **Output is always tailed + redacted.** Every `CargoOutput.stdout_tail` / `stderr_tail` is passed through `shipper_output_sanitizer::tail_lines`, which internally applies `redact_sensitive`. Callers can assume bearer tokens / `CARGO_REGISTRY_TOKEN=` values / `CARGO_REGISTRIES_<NAME>_TOKEN=` values are `[REDACTED]` before they ever reach `receipt.json` or the event log.
- **Redaction is idempotent** (see `redact_is_idempotent_*` tests).
- **Publish capture is bounded.** `cargo_publish` preserves its public result
  fields but now uses bounded capture: 256 KiB payload per stream, 64 KiB partial
  line, 4096 entries, 8 KiB reads. The engine/rehearsal consume the internal
  `cargo_publish_with_capture` result with typed completeness. Capacity/read loss
  suppresses both streams with a fixed summary (empty for output_lines=0), never
  a raw suffix. Complete requested tails preserve normal tail/redaction behavior.
  Redaction expansion is checked against the final payload limit too. Generic
  metadata, yank, install and dry-run capture retain their current semantics.
  Linux/macOS supervision separately bounds inherited-pipe drain and owned group
  cleanup. Deliberate group/session escape is outside that containment boundary;
  Windows bounded cleanup remains a separate obligation.
- **Registry identity.** Publish wrappers pass `--registry` only for non-empty non-`crates-io` names. Containment via `cargo_yank` always passes the exact non-empty registry name, including `crates-io`, so Cargo's `registry.default` cannot retarget the action; empty names are refused before spawning.
- **`WorkspaceMetadata::is_publishable`** treats version `0.0.0` as non-publishable and `publish = []` as non-publishable, matching Cargo's own semantics.
- **`topological_order`** is a DFS-based visitor (distinct from the Kahn/BTreeSet sort in `crate::plan`). It's still useful for diagnostics; production planning goes through `crate::plan::build_release_plan`.

## Architectural notes

- Layer-1 pure I/O. Must not import from `engine`, `plan`, `state`, or `runtime` (enforced by `.github/workflows/architecture-guard.yml`).
- Depends on `crate::ops::process` for the timeout-aware subprocess primitive; all subprocess spawning goes through there so Windows/Unix timeout handling is unified.
- External deps: `anyhow`, `cargo_metadata`, `serde`, `shipper_output_sanitizer`.
