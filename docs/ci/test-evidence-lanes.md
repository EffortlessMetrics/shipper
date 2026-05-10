# Test Evidence Lanes

This document maps the test evidence strategy for `shipper`: which lanes run when, what each lane proves, and how they compose into a complete evidence picture.

## Doctrine

```
PRs:     ripr + normal gates + targeted mutation when risk warrants it
Nightly: deeper mutation / fuzz / proptest lanes
Release: publish / readiness / security proof must be clean to ship
```

`ripr` is the PR-time exposure filter: a fast signal on which mutants are reachable from changed code. Full mutation belongs in targeted, nightly, and release lanes rather than on every PR.

## Lane Map

### Always-On (Every PR and Push)

| Job | What it proves |
|---|---|
| `fmt` | Code is formatted per `rustfmt` rules. |
| `clippy` | No Clippy warnings (treated as errors via `-D warnings`). |
| `test` (nextest, Linux/Windows/macOS) | Unit and integration tests pass on all three platforms. |
| `doc-tests` | Documentation examples compile and pass. |
| `MSRV gate` | Compiles and tests pass on the declared minimum Rust version. |
| `security` | `cargo audit` finds no known-vulnerable dependencies. |
| `architecture-guard` | Crate dependency boundaries are respected. |
| `BDD smoke` | Core workflow Cucumber scenarios pass. |

### Policy Gates (Added in This Rollout)

| Job | What it proves | PR introduced |
|---|---|---|
| `lint-policy` | Clippy ledger aligns with Cargo.toml and clippy.toml. | PR 5 |
| `no-panic-check` | No panic-family debt was added since baseline. | PR 8 |
| `file-policy` | All non-Rust files are receipted (advisory initially). | PR 9 |

### Advisory / Routed

| Job | Trigger | What it proves |
|---|---|---|
| `coverage` | main / dispatch / `coverage` / `full-ci` labels | Line/branch coverage, Codecov integration. |
| `ripr` | PRs touching `crates/**`, `xtask/**`, policy files | Reachable mutant exposure from changed code (advisory). |

### Nightly and Scheduled

| Job | Schedule | What it proves |
|---|---|---|
| `fuzz` | Nightly | No panic/OOM on fuzz inputs targeting state, events, output sanitizer. |
| `mutation` (full) | Nightly / `full-ci` label | Mutation score across trust-critical crates. |
| `crypto-proptests-heavy` | Nightly / `full-ci` label | Extended property-based tests for encrypt/decrypt round-trips. |

### Targeted Mutation (PR-Triggered)

Full mutation runs on a PR when:
- PR carries the `mutation` or `full-ci` label.
- Changes touch: `shipper-core` publish/reconcile/readiness, `shipper-encrypt`, `shipper-output-sanitizer`, `shipper-cargo-failure`, `shipper-sparse-index`, `shipper-webhook`, state/event/receipt types.
- `ripr` reports a severe reachable weak-coverage exposure.

### Release Proof

The release workflow proves end-to-end publication safety:

| Step | What it proves |
|---|---|
| `cargo xtask package-surface` | All workspace versions align; façade shape is intact. |
| `cargo xtask policy-report` | All policy gates are green. |
| `cargo xtask check-lint-policy` | Clippy ledger MSRV matches declared MSRV. |
| `cargo xtask check-no-panic-family` | No new panic-family debt. |
| `cargo xtask check-file-policy --mode blocking-allowlist` | All non-Rust files receipted. |
| `shipper plan` | Plan ID generated, publish order validated. |
| `shipper preflight` | Git clean, registry reachable, version unique, ownership valid. |
| `shipper publish` | Crates published in topological order with retry/backoff. |
| `shipper resume` (if interrupted) | State loaded from `.shipper/`, plan ID matched, publishes skipped. |
| Registry visibility check | Each crate confirmed visible before next crate starts. |
| Binary artifact build | Linux/Windows/macOS release binaries produced. |
| GitHub Release creation | Release notes and binaries attached to the tag. |

## Evidence Composition

A complete evidence picture for a release requires all of the following:

| Evidence | Source |
|---|---|
| Tests pass on all platforms | Three-OS nextest matrix |
| No known vulnerabilities | `cargo audit` |
| No architectural drift | Architecture guard |
| Format clean | `rustfmt` |
| Clippy clean | Clippy with `-D warnings` |
| MSRV verified | Separate MSRV job |
| BDD scenarios pass | Cucumber |
| No panic-family debt added | no-panic check |
| Policy gates green | lint-policy, file-policy |
| Publish dry-run succeeds | `cargo publish --dry-run` for all 13 crates |
| State artifacts valid | `.shipper/state.json`, `events.jsonl`, `receipt.json` reviewed |
| Trusted Publishing configured | OIDC token exchange verified |

## Trust-Critical Crates

These crates receive the most rigorous mutation coverage because they handle real registry, state, and security operations:

| Crate | Risk |
|---|---|
| `shipper-core` | Publish engine, reconcile, resume, plan |
| `shipper-types` | Shared state/event types |
| `shipper-encrypt` | Token encryption |
| `shipper-output-sanitizer` | Token redaction in logs |
| `shipper-cargo-failure` | Cargo exit-code / stderr classification |
| `shipper-sparse-index` | Registry sparse-index parsing |
| `shipper-registry` | Registry API interactions |
| `shipper-cli` | CLI dispatch, output |
| `shipper` | Install façade |
