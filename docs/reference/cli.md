# CLI reference

**Canonical source:** `shipper --help` and `shipper <subcommand> --help`. The help output is generated from the same source as the CLI behavior, so it never drifts.

This page is a topical map, not an exhaustive flag listing. For exhaustive flags, use `--help`.

## First-run command chain

Use the `shipper` facade binary for the user-facing workflow:

```bash
shipper doctor
shipper plan
shipper status
shipper preflight
```

`doctor` catches local setup blockers, `plan` shows what would publish,
`status` compares local versions to the registry, and `preflight` gives the
release-readiness verdict.

For CI, internal developer portals, or agent consumers, `shipper doctor`,
`shipper plan`, `shipper status`, `shipper preflight`, `shipper publish`,
and `shipper resume` support `--format json`. `publish` and `resume` emit
command-owned JSON envelopes with artifact paths and nested release receipt
evidence for each targeted registry.

## Commands at a glance

| Command | What it does | Writes state? |
|---|---|---|
| `shipper plan` | Compute and print the deterministic publish order | No |
| `shipper preflight` | Run safety checks without publishing | No (emits events) |
| `shipper publish` | Publish missing workspace versions; skip already-published `name@version` pairs | Yes |
| `shipper resume` | Continue from the last persisted state | Yes |
| `shipper rehearse` | Publish the plan to an alternate registry and verify it there | Yes (`rehearsal.json`, events) |
| `shipper status` | Compare local workspace versions to the registry | No |
| `shipper doctor` | Environment / auth / connectivity diagnostics | No |
| `shipper inspect-events` | View the event log | No |
| `shipper inspect-receipt` | View the end-of-run receipt | No |
| `shipper clean` | Clean `.shipper/` state files | Yes (destructive) |
| `shipper yank` | Yank one `crate@version`, or execute a reviewed yank plan — containment, not undo | Yes (events; registry-mutating) |
| `shipper plan-yank` | Generate a reverse-topological yank plan from a receipt | No (prints the plan) |
| `shipper fix-forward` | Generate a supersession plan from a receipt marked compromised | No (prints the plan) |
| `shipper remediate` | Generate or execute a receipt-driven remediation plan | Yes (`remediation-plan.json`; `--execute-plan` yanks) |
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

For canonical command-level behavior, `shipper --help` remains authoritative.
The table below documents the stable CI contract surfaces for idempotent
workspace publishing and recovery.

| Command | Scenario | Exit |
|---|---|---:|
| `shipper publish` | All package versions already exist | `0` |
| `shipper publish` | Mixed skipped-existing and successful publishes | `0` |
| `shipper publish` | Permanent publish failure | non-zero |
| `shipper publish` | Retry budget exhausted | non-zero |
| `shipper publish` | Ambiguous cargo result reconciled to published | `0` |
| `shipper publish` | Ambiguous cargo result remains unknown | non-zero |
| `shipper resume` | All packages already terminal | `0` |
| `shipper resume` | Plan/state mismatch | non-zero unless forced |
| `shipper status` | Mixed registry state read succeeds | `0` |
| `shipper status` | Registry/query failure | non-zero |

### The publish / resume exit vocabulary

`publish` and `resume` distinguish their two failure shapes, so a CI job can
branch without parsing output:

| Code | Meaning | What CI should do |
|---:|---|---|
| `0` | Every package reached a successful terminal state (published or already present) | proceed |
| `2` | At least one package did not — the run finalized a receipt | `shipper resume` after fixing the cause |
| `1` | The run failed before finalizing a receipt (auth, plan, lock, config) | fix the cause, rerun |

Exit `2` means a receipt exists and resuming is the intended recovery, *not*
that the invocation was rejected.

Note that `2` covers "all packages failed" as well as "some failed":
finalization classifies every non-all-successful receipt as
`PartialFailure`, so `CompleteFailure` (`1`) is not reachable from a
finalized run today. Treat `1` as "the command errored out", not as "all
packages failed". Read `state.json` or `events.jsonl` if you need the
per-package breakdown.

With `--registries` / `--all-registries`, the exit code reflects the
**worst** outcome across every targeted registry, so a partial failure on
one registry is not masked by a success on another.

**Caveat:** argument-parsing errors (unknown flag, missing subcommand,
invalid `--format` value) also exit `2`, because that is clap's convention
and no work is performed.

The two are distinguishable by side effect: a usage error writes nothing,
so a partial failure always leaves a `state.json` and an `events.jsonl`
behind. Check the right directory, though — that is `<workspace_root>/.shipper/`,
where `workspace_root` comes from `--manifest-path` (default `Cargo.toml`)
and **not** from the current working directory, unless `--state-dir` was
given an absolute path. A CI step that looks for `./.shipper/` after
running Shipper from a different directory will mis-classify the exit code.

**Multi-registry runs write elsewhere again.** When more than one registry
is targeted, each run gets its own subdirectory — `<state_dir>/<registry-name>/`
— so the root `state_dir` holds no `state.json` at all. A CI step checking
only the root would read "no state file" and mis-classify a partial failure
as a usage error, which is the opposite of the truth. Inspect
`<state_dir>/<registry-name>/` for every targeted registry.

If your pipeline needs the codes to be unambiguous, validate flags in a
separate step, or pass an explicit absolute `--state-dir` and test for that
path.

## See also

- [Tutorial: First publish](../tutorials/first-publish.md)
- [How-to: Run in GitHub Actions](../how-to/run-in-github-actions.md)
- [`.shipper.toml` reference](../configuration.md)
- [Failure modes](../failure-modes.md)
