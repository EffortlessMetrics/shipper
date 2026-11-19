# Layer: `ops::process` (process execution)

**Position:** Layer 1 (ops). Crate-private submodule of `shipper-core`.

## Single responsibility

Cross-platform command execution: spawn child processes, capture stdout/stderr,
apply optional timeouts, and shell out to `cargo`. Used by `ops::cargo` to
invoke `cargo publish`, `cargo package`, and friends with retry-friendly
result types.

## What lives here

- `types` — `CommandResult` and `CommandOutput` result types (serde-friendly).
- `run` — Basic command runners: `run_command`, `run_command_in_dir`,
  `run_command_with_env`, `run_command_streaming`, `run_command_simple`.
- `timeout` — generic timed execution and bounded publish capture. On Linux and
  macOS, nonblocking pipe reads share a polling loop with an invocation-owned
  process group. Windows uses a Job Object and nonwaiting named-pipe reads.
  Both have a separate one-second cleanup allowance. Other platforms retain
  the direct-child/blocking-reader implementation.
- `capture` — full capture or opt-in publish diagnostic tails. Publish readers
  use 8 KiB chunks, a 64 KiB partial-line limit, a 256 KiB retained/rendered
  payload limit per stream, and at most 4096 entries. Never allocate from an
  unchecked caller line count. Normal last-N eviction is not loss; capacity/read
  loss is sticky and clears fragments. Frame CR/LF and UTF-8 boundaries before
  rendering. The Cargo adapter suppresses both streams on either loss.
- `which` — `command_exists`/`which` helpers delegating to the `which` crate.
- `cargo` — `run_cargo`, `run_cargo_in_dir`, `cargo_dry_run`, `cargo_publish`
  convenience wrappers.

## Import rules

`ops::process` is Layer 1:
- MUST NOT import from `crate::engine`, `crate::plan`, `crate::state`, `crate::runtime`.
- MAY import from `crate::types` and external crates.

## Visibility

This subsystem is crate-private. Callers (primarily `ops::cargo`) consume it via
the `ops::process` facade. The hidden `engine::PublishInterruption` re-export is
an unsupported bridge for the separate executable-only CLI entry point; it owns
process-lifetime flags and leases but installs no OS handlers.

`run_command_with_timeout` keeps complete stdout/stderr. Only the explicit
`run_command_with_publish_capture` entry point bounds retention, including when
timeout is None. Timed generic execution and all bounded publish execution share
the platform supervisor; generic timeout=None keeps the existing
run_command_in_dir full-output route. Do not
implement bounded capture by reading everything and truncating afterward, and
do not route metadata or generic structured stdout through diagnostic tails.
Linux/macOS cleanup observes exit with `waitid(NOWAIT)` so the child pins its
process-group identity until one group termination signal. Reap with nonblocking
polls; after reaping, only read-only group probes are permitted. Return after the
one-second cleanup allowance even if inherited pipe EOF or group disappearance
cannot be established, closing owned readers without leaving blocked threads.
`cleanup_failure` is independent of `timed_out` and capture loss. It must not
authorize another upload. Early exit 0/42 keeps that exit and `timed_out=false`.
Group-signal failure is provisional: confirmed reap, group absence and both EOFs
prove cleanup complete even when an exited leader's group rejects the signal.

The explicit process-only CLI signal context supplies an interruption flag and
conditional-default flag. The supervisor holds a counted lease before spawn;
every result retains that lease through the engine's immediate outcome decision.
Interrupted/uncertain outcomes retain it through persistence; ordinary success
retains it through the Uploaded write, then releases before registry/backoff waits.
Core entry points
never install handlers, and interruption is separate from cleanup failure: proved
cleanup plus interruption preserves ambiguous attempt evidence without a permanent
cleanup hold. No request reset, handler unregistration, or deadline extension is
allowed between commands.

On Linux, zombie descendants can keep the group-absence probe positive when
their reparent target does not reap them. The supervisor conservatively retains
a hold even if no descendant is running. Containers should use an init process
that reaps orphaned children; zombie-only groups are not treated as absent.

The group contains cooperative descendants, not descendants that deliberately
change groups/sessions. An escaped pipe holder produces incomplete cleanup; an
escaped process that closes both pipes cannot be discovered by this mechanism.
Windows assigns the suspended child to an unnamed kill-on-close job before
resume. Direct-child status comes from its process handle. Job accounting and
pipe EOF independently confirm cleanup. The Windows polling path avoids Tokio
child-pipe adapters, which submit blocking reads on this platform.

## History

Absorbed from the standalone `shipper-process` microcrate (1948 LOC) in the
Phase 2 decrating effort. See `docs/decrating-plan.md` §6.
