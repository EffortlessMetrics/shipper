# CLI reference

**Canonical source:** `shipper --help` and `shipper <subcommand> --help`. The help output is generated from the same source as the CLI behavior, so it never drifts.

This page is a topical map, not an exhaustive flag listing. For exhaustive flags, use `--help`.

## Commands at a glance

| Command | What it does | Writes state? |
|---|---|---|
| `shipper plan` | Compute and print the deterministic publish order | No |
| `shipper preflight` | Run safety checks without publishing | No (emits events) |
| `shipper publish` | Execute the plan (the irreversible step) | Yes |
| `shipper resume` | Continue from the last persisted state | Yes |
| `shipper status` | Compare local workspace versions to the registry | No |
| `shipper doctor` | Environment / auth / connectivity diagnostics | No |
| `shipper inspect-events` | View the event log | No |
| `shipper inspect-receipt` | View the end-of-run receipt | No |
| `shipper clean` | Clean `.shipper/` state files | Yes (destructive) |
| `shipper config init` | Generate a default `.shipper.toml` | No |
| `shipper config validate` | Validate an existing config | No |
| `shipper completion <shell>` | Generate shell completion scripts | No |
| `shipper ci <platform>` | Print a CI workflow template | No |

## Most-used flags

### Global

- `--config <path>` — path to a custom `.shipper.toml`
- `--manifest-path <path>` — path to the workspace `Cargo.toml`
- `--registry <name>` — Cargo registry name (default `crates-io`)
- `--state-dir <path>` — directory for `.shipper/` state
- `--format <text|json>` — output format for structured commands
- `-v/--verbose`, `-q/--quiet` — verbosity controls

### Publish safety

- `--policy <safe|balanced|fast>` — verification posture
- `--verify-mode <workspace|package|none>` — dry-run granularity
- `--readiness-method <api|index|both>` — post-publish visibility check
- `--max-attempts <N>` — retry budget per crate (default 6)
- `--base-delay <duration>`, `--max-delay <duration>` — backoff envelope
- `--verify-timeout <duration>`, `--readiness-timeout <duration>` — verification budgets

### Preflight

- `--allow-dirty` — permit a dirty git working tree
- `--skip-ownership-check` — skip the owners preflight
- `--strict-ownership` — fail preflight on any ownership ambiguity

### Resume

- `--force-resume` — resume even if the computed plan differs from the state file (advanced; can cause duplicate publish attempts if misused)
- `--resume-from <crate>` — start from a specific crate

### Parallel

- `--parallel`, `--max-concurrent <N>` — parallelize within dependency levels
- `--per-package-timeout <duration>` — per-package timeout in parallel mode

## Policy matrix

| Policy | Verify mode | Readiness | Best for |
|---|---|---|---|
| `safe` (default) | `workspace` | `both` | Production releases |
| `balanced` | `package` | `api` | Regular releases when you want speed without skipping essentials |
| `fast` | `none` | none | Dev / sandbox registries only — not recommended for crates.io |

## Exit codes

(Exit-code reference is tracked separately; see `shipper --help` for the current list.)

## See also

- [Tutorial: First publish](../tutorials/first-publish.md)
- [How-to: Run in GitHub Actions](../how-to/run-in-github-actions.md)
- [`.shipper.toml` reference](../configuration.md)
- [Failure modes](../failure-modes.md)
